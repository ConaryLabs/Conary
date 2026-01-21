// tests/inference_benchmark.rs
//! Benchmark tests for capability inference performance and accuracy
//!
//! This module measures:
//! 1. Inference speed across different tiers
//! 2. Accuracy against known packages
//! 3. Memory usage for binary analysis

use conary::capability::inference::{
    infer_capabilities, BinaryAnalyzer, Confidence, HeuristicInferrer, InferenceOptions,
    PackageFile, PackageMetadataRef, WellKnownProfiles,
};
use std::fs;
use std::time::Instant;

/// Benchmark result for a single inference run (kept for future detailed reporting)
#[allow(dead_code)]
struct BenchmarkResult {
    package_name: String,
    tier_used: u8,
    elapsed_ms: f64,
    confidence: Confidence,
    detected_network: bool,
    detected_ports: Vec<String>,
}

/// Load a system binary if it exists
fn load_binary(path: &str) -> Option<Vec<u8>> {
    fs::read(path).ok()
}

/// Known packages with expected capabilities for accuracy testing
#[allow(dead_code)]
struct ExpectedCapabilities {
    name: &'static str,
    binary_path: &'static str,
    expected_needs_network: bool,
    expected_ports: &'static [&'static str],  // Reserved for future port detection tests
    expected_syscall_profile: Option<&'static str>,  // Reserved for future syscall profile tests
}

const KNOWN_PACKAGES: &[ExpectedCapabilities] = &[
    ExpectedCapabilities {
        name: "curl",
        binary_path: "/usr/bin/curl",
        expected_needs_network: true,
        expected_ports: &[], // Client, no listen ports
        expected_syscall_profile: Some("network-client"),
    },
    ExpectedCapabilities {
        name: "ls",
        binary_path: "/usr/bin/ls",
        expected_needs_network: false,
        expected_ports: &[],
        expected_syscall_profile: None,
    },
    ExpectedCapabilities {
        name: "grep",
        binary_path: "/usr/bin/grep",
        expected_needs_network: false,
        expected_ports: &[],
        expected_syscall_profile: Some("minimal"),
    },
    ExpectedCapabilities {
        name: "ssh",
        binary_path: "/usr/bin/ssh",
        expected_needs_network: true,
        expected_ports: &[], // Client, no listen ports
        expected_syscall_profile: Some("network-client"),
    },
];

#[test]
fn benchmark_wellknown_lookup() {
    let packages = [
        "nginx", "postgresql", "redis", "curl", "git", "docker", "systemd",
        "openssh-server", "mysql-server", "haproxy", "prometheus", "grafana",
    ];

    let start = Instant::now();
    let iterations = 10000;

    for _ in 0..iterations {
        for pkg in &packages {
            let _ = WellKnownProfiles::lookup(pkg);
        }
    }

    let elapsed = start.elapsed();
    let per_lookup_ns = elapsed.as_nanos() as f64 / (iterations * packages.len()) as f64;

    println!("\n=== Well-known Lookup Benchmark ===");
    println!("Lookups: {}", iterations * packages.len());
    println!("Total time: {:?}", elapsed);
    println!("Per lookup: {:.1} ns", per_lookup_ns);
    println!("Lookups/sec: {:.0}", 1_000_000_000.0 / per_lookup_ns);

    // Performance assertion: should be fast (< 10µs per lookup in debug mode)
    // In release mode this would be < 1µs
    assert!(
        per_lookup_ns < 10000.0,
        "Well-known lookup too slow: {:.1} ns (should be < 10µs)",
        per_lookup_ns
    );
}

#[test]
fn benchmark_heuristic_inference() {
    // Create a typical package file set
    let files = vec![
        PackageFile::new("/usr/sbin/myservice"),
        PackageFile::new("/etc/myservice/config.conf"),
        PackageFile::new("/var/log/myservice/service.log"),
        PackageFile::new("/var/lib/myservice/data"),
        PackageFile::new("/usr/share/man/man1/myservice.1.gz"),
        PackageFile::new("/usr/share/doc/myservice/README"),
    ];

    let metadata = PackageMetadataRef {
        name: "myservice-server".to_string(),
        version: "1.0.0".to_string(),
        dependencies: vec![
            "libssl3".to_string(),
            "libc6".to_string(),
            "libsystemd0".to_string(),
        ],
        ..Default::default()
    };

    let start = Instant::now();
    let iterations = 1000;

    for _ in 0..iterations {
        let _ = HeuristicInferrer::infer(&files, &metadata);
    }

    let elapsed = start.elapsed();
    let per_inference_us = elapsed.as_micros() as f64 / iterations as f64;

    println!("\n=== Heuristic Inference Benchmark ===");
    println!("Inferences: {}", iterations);
    println!("Files per package: {}", files.len());
    println!("Total time: {:?}", elapsed);
    println!("Per inference: {:.1} µs", per_inference_us);
    println!("Inferences/sec: {:.0}", 1_000_000.0 / per_inference_us);

    // Performance assertion: should complete in < 1ms per inference
    assert!(
        per_inference_us < 1000.0,
        "Heuristic inference too slow: {:.1} µs",
        per_inference_us
    );
}

#[test]
fn benchmark_binary_analysis() {
    // Load a few common binaries
    let binaries: Vec<_> = ["/usr/bin/ls", "/usr/bin/cat", "/usr/bin/echo"]
        .iter()
        .filter_map(|path| {
            load_binary(path).map(|content| PackageFile::with_content(*path, content))
        })
        .collect();

    if binaries.is_empty() {
        println!("Skipping binary analysis benchmark: no binaries found");
        return;
    }

    let file_refs: Vec<_> = binaries.iter().collect();

    let start = Instant::now();
    let iterations = 100;

    for _ in 0..iterations {
        let _ = BinaryAnalyzer::analyze_all(&file_refs);
    }

    let elapsed = start.elapsed();
    let per_analysis_ms = elapsed.as_millis() as f64 / iterations as f64;

    println!("\n=== Binary Analysis Benchmark ===");
    println!("Analyses: {}", iterations);
    println!("Binaries per analysis: {}", binaries.len());
    println!(
        "Total binary size: {} KB",
        binaries.iter().map(|f| f.size).sum::<u64>() / 1024
    );
    println!("Total time: {:?}", elapsed);
    println!("Per analysis: {:.1} ms", per_analysis_ms);

    // Performance assertion: should complete in < 100ms per analysis
    assert!(
        per_analysis_ms < 100.0,
        "Binary analysis too slow: {:.1} ms",
        per_analysis_ms
    );
}

#[test]
fn benchmark_full_pipeline() {
    let files = vec![
        PackageFile::new("/usr/sbin/testservice"),
        PackageFile::new("/etc/testservice/config.conf"),
        PackageFile::new("/var/log/testservice/service.log"),
    ];

    let metadata = PackageMetadataRef {
        name: "testservice".to_string(),
        version: "1.0.0".to_string(),
        dependencies: vec!["libssl3".to_string()],
        ..Default::default()
    };

    // Test different option presets
    let presets = [
        ("fast (tier 1-2)", InferenceOptions::fast()),
        ("default (tier 1-2)", InferenceOptions::default()),
        ("full (tier 1-4)", InferenceOptions::full_analysis()),
    ];

    println!("\n=== Full Pipeline Benchmark ===");

    for (name, options) in presets {
        let start = Instant::now();
        let iterations = 1000;

        for _ in 0..iterations {
            let _ = infer_capabilities(&files, &metadata, &options);
        }

        let elapsed = start.elapsed();
        let per_inference_us = elapsed.as_micros() as f64 / iterations as f64;

        println!(
            "{}: {:.1} µs/inference, {:.0} inferences/sec",
            name,
            per_inference_us,
            1_000_000.0 / per_inference_us
        );
    }
}

#[test]
fn test_accuracy_known_packages() {
    println!("\n=== Accuracy Test: Known Packages ===");

    let mut correct = 0;
    let mut total = 0;
    let mut results = Vec::new();

    for pkg in KNOWN_PACKAGES {
        // Test well-known profile
        if let Some(profile) = WellKnownProfiles::lookup(pkg.name) {
            total += 1;
            let network_match = !profile.network.no_network == pkg.expected_needs_network;
            if network_match {
                correct += 1;
            }

            results.push(format!(
                "{}: well-known profile, network={} (expected {}), {}",
                pkg.name,
                !profile.network.no_network,
                pkg.expected_needs_network,
                if network_match { "PASS" } else { "FAIL" }
            ));
        }

        // Test binary analysis if binary exists
        if let Some(content) = load_binary(pkg.binary_path) {
            let file = PackageFile::with_content(pkg.binary_path, content);
            let files = vec![&file];
            if let Ok(result) = BinaryAnalyzer::analyze_all(&files) {
                total += 1;
                let network_match = !result.network.no_network == pkg.expected_needs_network;
                if network_match {
                    correct += 1;
                }

                results.push(format!(
                    "{}: binary analysis, network={} (expected {}), {}",
                    pkg.name,
                    !result.network.no_network,
                    pkg.expected_needs_network,
                    if network_match { "PASS" } else { "FAIL" }
                ));
            }
        }
    }

    for result in &results {
        println!("  {}", result);
    }

    let accuracy = if total > 0 {
        (correct as f64 / total as f64) * 100.0
    } else {
        0.0
    };

    println!("\nAccuracy: {}/{} ({:.1}%)", correct, total, accuracy);

    // We expect at least 70% accuracy for known packages
    assert!(
        accuracy >= 70.0 || total == 0,
        "Accuracy too low: {:.1}%",
        accuracy
    );
}

#[test]
fn test_tier_selection() {
    println!("\n=== Tier Selection Test ===");

    // Package with well-known profile should use tier 1
    let nginx_metadata = PackageMetadataRef {
        name: "nginx".to_string(),
        version: "1.24.0".to_string(),
        ..Default::default()
    };
    let nginx_files = vec![PackageFile::new("/usr/sbin/nginx")];
    let nginx_result = infer_capabilities(&nginx_files, &nginx_metadata, &InferenceOptions::default()).unwrap();
    assert_eq!(nginx_result.tier_used, 1, "nginx should use tier 1 (well-known)");
    println!("nginx: tier {}, confidence {:?}", nginx_result.tier_used, nginx_result.confidence.primary);

    // Unknown package should use tier 2
    let unknown_metadata = PackageMetadataRef {
        name: "totally-unknown-package".to_string(),
        version: "1.0.0".to_string(),
        ..Default::default()
    };
    let unknown_files = vec![PackageFile::new("/usr/bin/unknown")];
    let unknown_result = infer_capabilities(&unknown_files, &unknown_metadata, &InferenceOptions::default()).unwrap();
    assert!(unknown_result.tier_used >= 2, "unknown package should use tier 2+");
    println!("unknown: tier {}, confidence {:?}", unknown_result.tier_used, unknown_result.confidence.primary);
}

#[test]
fn test_confidence_correlation() {
    println!("\n=== Confidence Correlation Test ===");

    // Well-known profiles should have high confidence
    let profiles = ["nginx", "postgresql", "redis", "curl"];
    for name in profiles {
        if let Some(profile) = WellKnownProfiles::lookup(name) {
            assert!(
                profile.confidence.primary >= Confidence::High,
                "{} should have high confidence, got {:?}",
                name,
                profile.confidence.primary
            );
            println!("{}: {:?}", name, profile.confidence.primary);
        }
    }

    // Heuristic inference with minimal evidence should have low/medium confidence
    let minimal_files = vec![PackageFile::new("/usr/bin/test")];
    let minimal_metadata = PackageMetadataRef {
        name: "test".to_string(),
        version: "1.0.0".to_string(),
        ..Default::default()
    };
    let minimal_result = HeuristicInferrer::infer(&minimal_files, &minimal_metadata).unwrap();
    println!(
        "Minimal heuristic: {:?}",
        minimal_result.confidence.primary
    );

    // Heuristic inference with more evidence should have higher confidence
    let rich_files = vec![
        PackageFile::new("/usr/sbin/myserver"),
        PackageFile::new("/etc/myserver/config.conf"),
        PackageFile::new("/var/log/myserver/server.log"),
        PackageFile::new("/var/lib/myserver/data"),
    ];
    let rich_metadata = PackageMetadataRef {
        name: "myserver-server".to_string(),
        version: "1.0.0".to_string(),
        dependencies: vec!["libssl3".to_string(), "libsystemd0".to_string()],
        ..Default::default()
    };
    let rich_result = HeuristicInferrer::infer(&rich_files, &rich_metadata).unwrap();
    println!("Rich heuristic: {:?}", rich_result.confidence.primary);

    // Rich evidence should produce higher or equal confidence
    assert!(
        rich_result.confidence.evidence_count >= minimal_result.confidence.evidence_count,
        "More files should produce more evidence"
    );
}

/// Summary benchmark that generates a report
#[test]
fn benchmark_summary() {
    println!("\n");
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║         Capability Inference Benchmark Summary               ║");
    println!("╠══════════════════════════════════════════════════════════════╣");

    // Well-known profiles
    let known_count = WellKnownProfiles::list_known_packages().len();
    println!("║ Well-known profiles: {:<40} ║", known_count);

    // Tier descriptions
    println!("║                                                              ║");
    println!("║ Inference Tiers:                                             ║");
    println!("║   1. Well-known profiles  - Fast, high confidence            ║");
    println!("║   2. Heuristics           - Fast, medium confidence          ║");
    println!("║   3. Config scanning      - Medium, medium confidence        ║");
    println!("║   4. Binary analysis      - Slow, high confidence            ║");

    println!("║                                                              ║");
    println!("║ Performance Targets:                                         ║");
    println!("║   Well-known lookup:  < 1 µs                                 ║");
    println!("║   Heuristic inference: < 1 ms                                ║");
    println!("║   Binary analysis:    < 100 ms                               ║");

    println!("╚══════════════════════════════════════════════════════════════╝");
}
