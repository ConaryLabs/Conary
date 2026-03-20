// conary-core/benches/erofs_build.rs

//! Benchmark EROFS image building to validate sub-second claim.
//! This is a quick measurement using Instant, not statistically rigorous.
//! For CI, consider adding criterion later.

use conary_core::generation::builder::{FileEntryRef, build_erofs_image};
use std::time::Instant;
use tempfile::TempDir;

fn generate_entries(count: usize) -> Vec<FileEntryRef> {
    (0..count)
        .map(|i| FileEntryRef {
            path: format!("usr/lib/file_{i:06}"),
            sha256_hash: format!("{i:064x}"),
            size: 4096,
            permissions: 0o644,
        })
        .collect()
}

fn main() {
    for count in [1_000, 10_000, 50_000, 100_000] {
        let entries = generate_entries(count);
        let tmp = TempDir::new().unwrap();
        let gen_dir = tmp.path().join("gen");
        std::fs::create_dir_all(&gen_dir).unwrap();

        let start = Instant::now();
        let result = build_erofs_image(&entries, &[], &gen_dir).unwrap();
        let elapsed = start.elapsed();

        println!(
            "{count:>7} files: {elapsed:>8.3?}  image_size={:.1}MB",
            result.image_size as f64 / 1_048_576.0
        );
    }
}
