// conary-core/benches/selected_root_delta.rs

//! Installed-path scaling for the selected-root transaction commit boundary.

use std::collections::BTreeMap;
use std::hint::black_box;
use std::time::Duration;

use conary_core::generation::root_manifest::{
    CapturedSelectedRoot, GENERATION_ROOT_MANIFEST_VERSION, GenerationRootEntry,
    GenerationRootManifest, MutableStateManifest, SELECTED_ROOT_MANIFEST_DELTA_VERSION,
    SelectedRootManifestDelta, SelectedRootSnapshot,
};
use conary_core::payload::{
    PayloadContentAuthority, PayloadIdentity, PayloadNode, PayloadNodeKind, PayloadTimestamp,
    ResolvedPayloadNode,
};
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

const INSTALLED_FILE_COUNTS: [usize; 3] = [1_000, 10_000, 100_000];

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

fn directory_entry(path: &str) -> GenerationRootEntry {
    GenerationRootEntry {
        path: path.to_string(),
        node: node(PayloadNodeKind::Directory, 0o755),
        content: None,
    }
}

fn regular_entry(path: String, content_identity: usize) -> GenerationRootEntry {
    GenerationRootEntry {
        path,
        node: node(
            PayloadNodeKind::Regular {
                hardlink_identity: None,
            },
            0o644,
        ),
        content: Some(PayloadContentAuthority {
            sha256: format!("{content_identity:064x}"),
            size: 4096,
        }),
    }
}

fn prior_with_installed_files(installed_files: usize) -> CapturedSelectedRoot {
    let mut entries = vec![directory_entry("/usr"), directory_entry("/usr/lib")];
    entries.extend(
        (0..installed_files)
            .map(|index| regular_entry(format!("/usr/lib/lib{index:06}.so"), index)),
    );
    let prior = CapturedSelectedRoot {
        generation: GenerationRootManifest {
            version: GENERATION_ROOT_MANIFEST_VERSION,
            root: node(PayloadNodeKind::Directory, 0o755),
            entries,
        },
        state: MutableStateManifest::empty(),
    };
    prior.generation.validate().unwrap();
    prior.state.validate().unwrap();
    prior
}

fn fixed_one_path_upsert() -> SelectedRootManifestDelta {
    SelectedRootManifestDelta {
        version: SELECTED_ROOT_MANIFEST_DELTA_VERSION,
        root: None,
        removals: Vec::new(),
        opaque_directories: Vec::new(),
        upserts: vec![regular_entry(
            "/usr/lib/lib000000.so".to_string(),
            usize::MAX,
        )],
    }
}

fn benchmark_installed_path_scaling(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("selected_root_delta/fixed_one_path_upsert");
    group.sample_size(20);
    group.warm_up_time(Duration::from_secs(2));
    group.measurement_time(Duration::from_secs(8));

    for installed_files in INSTALLED_FILE_COUNTS {
        let prior = prior_with_installed_files(installed_files);
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conary_core::db::schema::ensure_current(&conn).unwrap();
        let snapshot = SelectedRootSnapshot::capture(&conn, &prior).unwrap();
        let delta = fixed_one_path_upsert();
        delta.validate().unwrap();
        snapshot.apply_delta(&conn, &delta).unwrap();

        group.bench_with_input(
            BenchmarkId::from_parameter(installed_files),
            &installed_files,
            |bencher, _| {
                bencher.iter(|| {
                    black_box(
                        snapshot
                            .apply_delta(black_box(&conn), black_box(&delta))
                            .unwrap(),
                    )
                });
            },
        );
    }
    group.finish();
}

criterion_group!(benches, benchmark_installed_path_scaling);
criterion_main!(benches);
