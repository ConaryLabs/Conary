// crates/conary-core/src/ccs/chunking/tests.rs

use super::*;
use tempfile::TempDir;

#[test]
fn test_chunk_bytes_basic() {
    let chunker = Chunker::new();

    // Create some test data (needs to be larger than min chunk size)
    let data: Vec<u8> = (0..100_000).map(|i| (i % 256) as u8).collect();

    let chunks = chunker.chunk_bytes(&data);

    // Should have at least one chunk
    assert!(!chunks.is_empty());

    // Total length should equal original
    let total_len: u64 = chunks.iter().map(|c| u64::from(c.length)).sum();
    assert_eq!(total_len, data.len() as u64);

    // Chunks should be contiguous
    let mut offset = 0u64;
    for chunk in &chunks {
        assert_eq!(chunk.offset, offset);
        offset += u64::from(chunk.length);
    }
}

#[test]
fn chunk_exposes_read_only_authenticated_geometry() {
    let data = b"opaque chunk evidence";
    let chunks = Chunker::new().chunk_bytes(data);
    assert_eq!(chunks.len(), 1);
    let chunk = &chunks[0];

    assert_eq!(chunk.offset(), 0);
    assert_eq!(chunk.length(), data.len() as u32);
    assert_eq!(chunk.data(), data);
    assert_eq!(chunk.hash(), &crate::hash::sha256_bytes(data));
    assert_eq!(chunk.hash_hex(), crate::hash::sha256(data));
}

#[test]
fn test_same_content_same_chunks() {
    let chunker = Chunker::new();

    // Same data should produce same chunks
    let data: Vec<u8> = (0..100_000).map(|i| (i % 256) as u8).collect();

    let chunks1 = chunker.chunk_bytes(&data);
    let chunks2 = chunker.chunk_bytes(&data);

    assert_eq!(chunks1.len(), chunks2.len());
    for (c1, c2) in chunks1.iter().zip(chunks2.iter()) {
        assert_eq!(c1.hash, c2.hash);
    }
}

/// Generate pseudo-random data using a simple LCG
fn pseudo_random_data(seed: u64, len: usize) -> Vec<u8> {
    let mut x = seed;
    (0..len)
        .map(|_| {
            // LCG from Knuth MMIX
            x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            (x >> 32) as u8
        })
        .collect()
}

#[test]
fn test_small_change_few_chunks_differ() {
    let chunker = Chunker::new();

    // Create realistic pseudo-random test data
    let data1 = pseudo_random_data(42, 500_000);
    let mut data2 = data1.clone();

    // Change one byte near the middle
    data2[250_000] = data2[250_000].wrapping_add(1);

    let chunks1 = chunker.chunk_bytes(&data1);
    let chunks2 = chunker.chunk_bytes(&data2);

    // Count differing chunks
    let hashes1: std::collections::HashSet<_> = chunks1.iter().map(|c| c.hash).collect();
    let hashes2: std::collections::HashSet<_> = chunks2.iter().map(|c| c.hash).collect();

    let shared = hashes1.intersection(&hashes2).count();
    let different = hashes1.symmetric_difference(&hashes2).count();

    // CDC property: a single byte change should only affect ~1 chunk
    // Most chunks should be shared
    println!(
        "Chunks: {} original, {} modified, {} shared, {} different",
        chunks1.len(),
        chunks2.len(),
        shared,
        different
    );

    // At least half of chunks should be shared (conservative bound)
    assert!(
        shared >= chunks1.len().saturating_sub(5),
        "Most chunks should be shared: {} shared of {}",
        shared,
        chunks1.len()
    );
}

#[test]
fn test_chunk_store() {
    let temp_dir = TempDir::new().unwrap();
    let store = ChunkStore::new(temp_dir.path()).unwrap();
    let chunker = Chunker::new();

    let data: Vec<u8> = (0..100_000).map(|i| (i % 256) as u8).collect();
    let chunks = chunker.chunk_bytes(&data);

    // Store first chunk
    let chunk = &chunks[0];
    assert!(!store.has_chunk(&chunk.hash));

    let stored = store.store_chunk(chunk).unwrap();
    assert!(stored); // Should be newly stored
    assert!(store.has_chunk(&chunk.hash));

    // Store again - should be idempotent
    let stored_again = store.store_chunk(chunk).unwrap();
    assert!(!stored_again); // Already exists

    // Retrieve and verify
    let retrieved = store.get_chunk(&chunk.hash).unwrap();
    assert_eq!(retrieved, chunk.data);
}

#[test]
fn test_chunk_file() {
    let temp_dir = TempDir::new().unwrap();
    let test_file = temp_dir.path().join("test.bin");

    // Create test file
    let data: Vec<u8> = (0..100_000).map(|i| (i % 256) as u8).collect();
    std::fs::write(&test_file, &data).unwrap();

    let chunker = Chunker::new();
    let chunked = chunker.chunk_file(&test_file).unwrap();

    assert_eq!(chunked.size, data.len() as u64);
    assert!(!chunked.chunks.is_empty());

    // Verify file hash
    let expected_hash: [u8; 32] = crate::hash::sha256_bytes(&data);
    assert_eq!(chunked.file_hash, expected_hash);
}

#[test]
fn test_streaming_chunks_match_slice_boundaries_and_reassemble() {
    let data = pseudo_random_data(8_675_309, 1_250_000);
    let chunker = Chunker::new();
    let expected = chunker.chunk_bytes(&data);
    let mut streamed = Vec::new();

    let processed = chunker
        .visit_reader_chunks(std::io::Cursor::new(&data), |chunk| {
            streamed.push(chunk.clone());
            Ok(())
        })
        .unwrap();

    assert_eq!(processed, data.len() as u64);
    assert_eq!(streamed.len(), expected.len());
    for (streamed, expected) in streamed.iter().zip(expected.iter()) {
        assert_eq!(streamed.offset, expected.offset);
        assert_eq!(streamed.length, expected.length);
        assert_eq!(streamed.hash, expected.hash);
        assert_eq!(streamed.data, expected.data);
    }
    let reassembled: Vec<u8> = streamed.into_iter().flat_map(|chunk| chunk.data).collect();
    assert_eq!(reassembled, data);
}

#[test]
fn test_delta_calculation() {
    let chunker = Chunker::new();

    // Use pseudo-random data for realistic CDC behavior
    let data1 = pseudo_random_data(123, 500_000);
    let mut data2 = data1.clone();

    // Modify a small portion (simulates a code change)
    for item in data2.iter_mut().take(250_200).skip(250_000) {
        *item = 0xFF;
    }

    let temp_dir = TempDir::new().unwrap();
    let file1 = temp_dir.path().join("v1.bin");
    let file2 = temp_dir.path().join("v2.bin");

    std::fs::write(&file1, &data1).unwrap();
    std::fs::write(&file2, &data2).unwrap();

    let chunked1 = chunker.chunk_file(&file1).unwrap();
    let chunked2 = chunker.chunk_file(&file2).unwrap();

    let delta = calculate_delta(&chunked1, &chunked2);

    println!("Delta stats:");
    println!(
        "  Old: {} bytes, {} chunks",
        delta.old_size, delta.old_chunks
    );
    println!(
        "  New: {} bytes, {} chunks",
        delta.new_size, delta.new_chunks
    );
    println!(
        "  Shared: {} chunks, {} bytes",
        delta.shared_chunks, delta.shared_bytes
    );
    println!(
        "  Download: {} bytes ({:.1}% savings)",
        delta.download_size(),
        delta.savings_ratio() * 100.0
    );

    // Should have significant savings - a 200-byte change in 500KB
    // should only affect 1-2 chunks
    assert!(
        delta.savings_ratio() > 0.5,
        "Should share >50% of content: {:.1}% savings",
        delta.savings_ratio() * 100.0
    );
}

#[test]
fn test_reassemble() {
    let temp_dir = TempDir::new().unwrap();
    let store = ChunkStore::new(&temp_dir.path().join("chunks")).unwrap();
    let chunker = Chunker::new();

    // Create and chunk test data
    let data: Vec<u8> = (0..100_000).map(|i| (i % 256) as u8).collect();
    let chunks = chunker.chunk_bytes(&data);

    // Store all chunks
    for chunk in &chunks {
        store.store_chunk(chunk).unwrap();
    }

    // Reassemble
    let hashes: Vec<[u8; 32]> = chunks.iter().map(|c| c.hash).collect();
    let reassembled = store.reassemble(&hashes).unwrap();

    assert_eq!(reassembled, data);
}

#[test]
fn test_store_stats() {
    let temp_dir = TempDir::new().unwrap();
    let store = ChunkStore::new(&temp_dir.path().join("chunks")).unwrap();
    let chunker = Chunker::new();

    // Create test file
    let test_file = temp_dir.path().join("test.bin");
    let data: Vec<u8> = (0..100_000).map(|i| (i % 256) as u8).collect();
    std::fs::write(&test_file, &data).unwrap();

    let chunked = chunker.chunk_file(&test_file).unwrap();

    // First store - all new
    let stats1 = store.store_chunked_file(&chunked).unwrap();
    assert_eq!(stats1.existing_chunks, 0);
    assert!(stats1.new_chunks > 0);
    assert_eq!(stats1.dedup_ratio(), 0.0);

    // Second store - all deduped
    let stats2 = store.store_chunked_file(&chunked).unwrap();
    assert_eq!(stats2.new_chunks, 0);
    assert!(stats2.existing_chunks > 0);
    assert!((stats2.dedup_ratio() - 1.0).abs() < 0.01);
}

#[test]
fn test_get_chunk_rejects_corrupted_data() {
    let temp_dir = TempDir::new().unwrap();
    let store = ChunkStore::new(temp_dir.path()).unwrap();
    let chunk = Chunk {
        hash: crate::hash::sha256_bytes(b"expected"),
        offset: 0,
        length: 8,
        data: b"expected".to_vec(),
    };

    let chunk_path = store.chunk_path(&chunk.hash);
    std::fs::create_dir_all(chunk_path.parent().unwrap()).unwrap();
    std::fs::write(&chunk_path, b"corrupted").unwrap();

    let err = store.get_chunk(&chunk.hash).unwrap_err();
    assert!(err.to_string().contains("hash"));
}

/// Test CDC with the conary binary itself (run with --ignored)
#[test]
#[ignore]
fn test_cdc_real_binary() {
    use std::collections::HashSet;

    let binary_path = std::path::Path::new("target/release/conary");
    if !binary_path.exists() {
        println!("Skipping: binary not found at {:?}", binary_path);
        return;
    }

    let mut data = Vec::new();
    File::open(binary_path)
        .unwrap()
        .read_to_end(&mut data)
        .unwrap();
    println!("\nTesting CDC on real binary:");
    println!("  Binary: {:?}", binary_path);
    println!(
        "  Size: {} bytes ({:.2} MB)",
        data.len(),
        data.len() as f64 / 1_048_576.0
    );

    let chunker = Chunker::new();
    let chunks = chunker.chunk_bytes(&data);

    println!("\n  Chunk distribution:");
    println!("  Total chunks: {}", chunks.len());

    let sizes: Vec<u32> = chunks.iter().map(|c| c.length).collect();
    let total: u64 = sizes.iter().map(|&s| s as u64).sum();
    let avg = total as usize / sizes.len();
    let min = *sizes.iter().min().unwrap() as usize;
    let max = *sizes.iter().max().unwrap() as usize;

    println!("  Min chunk: {} bytes ({:.1} KB)", min, min as f64 / 1024.0);
    println!("  Max chunk: {} bytes ({:.1} KB)", max, max as f64 / 1024.0);
    println!("  Avg chunk: {} bytes ({:.1} KB)", avg, avg as f64 / 1024.0);

    // Simulate a small change (like a version string update)
    let mut modified = data.clone();
    if modified.len() > 10000 {
        // Change a few bytes to simulate a patch
        for item in modified.iter_mut().take(10010).skip(10000) {
            *item = item.wrapping_add(1);
        }
    }

    let chunks2 = chunker.chunk_bytes(&modified);

    // Compare hashes
    let hashes1: HashSet<_> = chunks.iter().map(|c| c.hash).collect();
    let hashes2: HashSet<_> = chunks2.iter().map(|c| c.hash).collect();

    let shared = hashes1.intersection(&hashes2).count();
    let savings = shared as f64 / chunks.len() as f64;
    let download_chunks = chunks.len().saturating_sub(shared);
    let download_bytes: u64 = chunks2
        .iter()
        .filter(|c| !hashes1.contains(&c.hash))
        .map(|c| c.length as u64)
        .sum();

    println!("\n  Simulated 10-byte patch:");
    println!("    Original chunks: {}", chunks.len());
    println!("    Modified chunks: {}", chunks2.len());
    println!("    Shared chunks: {} ({:.1}%)", shared, savings * 100.0);
    println!(
        "    Would download: {} chunks, {} bytes ({:.1} KB)",
        download_chunks,
        download_bytes,
        download_bytes as f64 / 1024.0
    );
    println!(
        "    Bandwidth savings: {:.1}%",
        (1.0 - download_bytes as f64 / data.len() as f64) * 100.0
    );

    // CDC should provide significant savings
    assert!(
        savings > 0.9,
        "Should share >90% of chunks for small change"
    );
}
