// conary-core/benches/erofs_build.rs

use std::collections::BTreeMap;

use conary_core::generation::root_manifest::{
    GENERATION_ROOT_MANIFEST_VERSION, GenerationRootEntry, GenerationRootManifest,
    build_erofs_image_from_root_manifest,
};
use conary_core::payload::{
    PayloadContentAuthority, PayloadIdentity, PayloadNode, PayloadNodeKind, PayloadTimestamp,
    ResolvedPayloadNode,
};
use criterion::{Criterion, criterion_group, criterion_main};

fn node(kind: PayloadNodeKind, permissions: u32) -> ResolvedPayloadNode {
    let mode_type = match kind {
        PayloadNodeKind::Regular { .. } | PayloadNodeKind::Hardlink { .. } => libc::S_IFREG,
        PayloadNodeKind::Directory => libc::S_IFDIR,
        PayloadNodeKind::Symlink { .. } => libc::S_IFLNK,
        PayloadNodeKind::BlockDevice { .. } => libc::S_IFBLK,
        PayloadNodeKind::CharacterDevice { .. } => libc::S_IFCHR,
        PayloadNodeKind::Fifo => libc::S_IFIFO,
        PayloadNodeKind::Socket => libc::S_IFSOCK,
    };
    ResolvedPayloadNode {
        source: PayloadNode {
            kind,
            mode: mode_type | permissions,
            user: PayloadIdentity::Numeric { id: 0 },
            group: PayloadIdentity::Numeric { id: 0 },
            mtime: PayloadTimestamp::UNIX_EPOCH,
            xattrs: BTreeMap::new(),
        },
        uid: 0,
        gid: 0,
    }
}

fn generate_manifest(count: usize) -> GenerationRootManifest {
    let mut entries = vec![
        GenerationRootEntry {
            path: "/usr".to_string(),
            node: node(PayloadNodeKind::Directory, 0o755),
            content: None,
        },
        GenerationRootEntry {
            path: "/usr/lib".to_string(),
            node: node(PayloadNodeKind::Directory, 0o755),
            content: None,
        },
    ];
    entries.extend((0..count).map(|index| GenerationRootEntry {
        path: format!("/usr/lib/lib{index:06}.so"),
        node: node(
            PayloadNodeKind::Regular {
                hardlink_identity: None,
            },
            0o644,
        ),
        content: Some(PayloadContentAuthority {
            sha256: format!("{index:064x}"),
            size: 4096,
        }),
    }));
    GenerationRootManifest {
        version: GENERATION_ROOT_MANIFEST_VERSION,
        root: node(PayloadNodeKind::Directory, 0o755),
        entries,
    }
}

fn benchmark_erofs_build(criterion: &mut Criterion) {
    let manifest = generate_manifest(10_000);
    criterion.bench_function("erofs_build_10k_exact_nodes", |bencher| {
        bencher.iter(|| {
            let temp = tempfile::tempdir().unwrap();
            build_erofs_image_from_root_manifest(&manifest, temp.path()).unwrap()
        });
    });
}

criterion_group!(benches, benchmark_erofs_build);
criterion_main!(benches);
