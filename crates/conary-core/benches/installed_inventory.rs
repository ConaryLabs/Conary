// conary-core/benches/installed_inventory.rs

use conary_core::packages::{
    InstalledFileAbsencePolicy, InstalledFileInfo, InstalledInventoryFile,
    InstalledInventoryPackage, InstalledInventorySnapshot, InstalledInventoryWork,
    InstalledPackageIdentity, NativeInstallReason, SystemPackageManager,
};
use conary_core::repository::dependency_model::DebianMultiArch;
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use std::hint::black_box;

fn inventory_packages(package_count: usize) -> Vec<InstalledInventoryPackage> {
    (0..package_count)
        .map(|index| {
            let name = format!("fixture-{index:05}");
            let files = [
                format!("/usr/bin/{name}"),
                format!("/usr/lib/{name}.so"),
                format!("/usr/share/doc/{name}/README"),
                "/usr/share/doc".to_string(),
            ]
            .into_iter()
            .map(|path| InstalledInventoryFile {
                file: InstalledFileInfo {
                    path,
                    size: 64,
                    mode: i32::try_from(libc::S_IFREG | 0o644).unwrap(),
                    digest: None,
                    user: None,
                    group: None,
                    link_target: None,
                    mtime: None,
                    absence_policy: InstalledFileAbsencePolicy::Required,
                },
                config: None,
            })
            .collect();
            InstalledInventoryPackage {
                identity: InstalledPackageIdentity::dpkg(
                    &name,
                    &name,
                    "1.0-1",
                    "amd64",
                    DebianMultiArch::No,
                )
                .unwrap(),
                description: None,
                install_reason: NativeInstallReason::Dependency,
                files,
                requirements: Vec::new(),
                provides: Vec::new(),
                lifecycle: Vec::new(),
            }
        })
        .collect()
}

fn benchmark_inventory_projection(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("installed_inventory/coherent_projection");
    for (label, package_count) in [("small", 100), ("distribution", 2_500)] {
        group.bench_with_input(
            BenchmarkId::new(label, package_count),
            &package_count,
            |bencher, package_count| {
                bencher.iter(|| {
                    let snapshot = InstalledInventorySnapshot::from_packages(
                        SystemPackageManager::Dpkg,
                        inventory_packages(*package_count),
                        InstalledInventoryWork {
                            manager_processes: 2,
                            database_bulk_reads: 2,
                            ownership_records: u64::try_from(*package_count * 4).unwrap(),
                        },
                    )
                    .unwrap();
                    black_box(snapshot.token().as_str());
                    black_box(snapshot.owners_by_path().len());
                });
            },
        );
    }
    group.finish();
}

criterion_group!(benches, benchmark_inventory_projection);
criterion_main!(benches);
