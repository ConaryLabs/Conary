// apps/conary/src/commands/ccs/build/render.rs
//! Render package authoring reports.

use conary_core::ccs::builder::BuildResult;
use conary_core::ccs::native_export::LossReport;

/// Print a concise build summary.
pub(super) fn print_build_summary(result: &BuildResult) {
    println!();
    println!("Build Summary");
    println!("=============");
    println!();
    println!(
        "Package: {} v{}",
        result.manifest.package.name, result.manifest.package.version
    );
    println!("Total files: {}", result.files.len());
    println!("Total size: {} bytes", result.total_size);
    println!(
        "Payload sources: {} regular files",
        result
            .payloads
            .iter()
            .filter(|payload| payload.node.kind.is_regular())
            .count()
    );

    if let Some(ref stats) = result.chunk_stats {
        println!();
        println!("CDC Chunking:");
        println!("  Chunked files: {} (files >16KB)", stats.chunked_files);
        println!("  Whole files: {} (files ≤16KB)", stats.whole_files);
        println!("  Total chunks: {}", stats.total_chunks);
        println!("  Unique chunks: {}", stats.unique_chunks);
        if stats.dedup_savings > 0 {
            println!("  Intra-package dedup: {} bytes saved", stats.dedup_savings);
        }
    }

    println!();
    println!("Components:");

    let mut comp_names: Vec<_> = result.components.keys().collect();
    comp_names.sort();

    for name in comp_names {
        let comp = &result.components[name];
        println!(
            "  :{} - {} files ({} bytes)",
            name,
            comp.files.len(),
            comp.size
        );
    }
}

pub(super) fn print_loss_report(report: &LossReport, format_name: &str) {
    if report.is_empty() {
        return;
    }
    println!("  Conversion notes for {format_name}:");
    for note in &report.unsupported_features {
        crate::ui::row(crate::ui::Status::Warn, &["Unsupported", note]);
    }
    for note in &report.hook_notes {
        crate::ui::row(crate::ui::Status::Info, &["Hook", note]);
    }
    for note in &report.dependency_notes {
        crate::ui::row(crate::ui::Status::Info, &["Dependency", note]);
    }
}
