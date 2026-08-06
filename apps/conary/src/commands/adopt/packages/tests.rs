// apps/conary/src/commands/adopt/packages/tests.rs

use std::fs;
use std::path::PathBuf;

use conary_core::db;
use conary_core::db::models::{FileEntry, InstallSource, ProvideEntry, Trove, TroveType};
use conary_core::payload::{PayloadContentAuthority, PayloadNodeKind};
use walkdir::WalkDir;

use super::*;

#[derive(Default)]
struct FixtureSource {
    lookups: HashMap<String, PackageLookup>,
    files: HashMap<String, Vec<FileInfoTuple>>,
    requirements: HashMap<String, Vec<RepositoryRequirementGroup>>,
    provides: HashMap<String, Vec<ProvidedCapability>>,
}

impl FixtureSource {
    fn with_ready(self, requested: &str, name: &str, file: FileInfoTuple) -> Self {
        self.with_ready_arch(requested, name, "x86_64", file)
    }

    fn with_ready_arch(
        mut self,
        requested: &str,
        name: &str,
        architecture: &str,
        file: FileInfoTuple,
    ) -> Self {
        let selector = format!("{name}-1.2.3-4.{architecture}");
        self.lookups.insert(
            requested.to_string(),
            PackageLookup::Found(ResolvedNativePackage {
                requested: requested.to_string(),
                native: InstalledPackageIdentity::rpm(
                    &selector,
                    name,
                    None,
                    "1.2.3",
                    "4",
                    architecture,
                )
                .unwrap(),
                description: Some("Fixture package".to_string()),
            }),
        );
        self.files.insert(selector.clone(), vec![file]);
        self.requirements.insert(
            selector.clone(),
            vec![
                conary_core::repository::requirement::parse_native_requirement(
                    conary_core::repository::dependency_model::RepositoryRequirementKind::Depends,
                    conary_core::repository::versioning::VersionScheme::Rpm,
                    "libc >= 2",
                )
                .unwrap(),
            ],
        );
        self.provides.insert(
            selector,
            vec![
                ProvidedCapability {
                kind: conary_core::repository::dependency_model::RepositoryCapabilityKind::PackageName,
                name: name.to_string(),
                version: Some("1.2.3-4".to_string()),
                version_relation: Some(
                    conary_core::repository::dependency_model::ProvideVersionRelation::Equal,
                ),
                version_scheme: VersionScheme::Rpm,
                architecture_qualifier: conary_core::repository::dependency_model::ProvideArchitectureQualifier::Implicit,
                provenance: conary_core::repository::dependency_model::CapabilityProvenance::ExactIdentity,
                },
                ProvidedCapability {
                    kind: conary_core::repository::dependency_model::RepositoryCapabilityKind::Generic,
                    name: format!("config({name})"),
                    version: Some("1.2.3-4".to_string()),
                    version_relation: Some(
                        conary_core::repository::dependency_model::ProvideVersionRelation::Equal,
                    ),
                    version_scheme: VersionScheme::Rpm,
                    architecture_qualifier: conary_core::repository::dependency_model::ProvideArchitectureQualifier::Implicit,
                    provenance: conary_core::repository::dependency_model::CapabilityProvenance::SourceDeclared {
                        format: conary_core::repository::dependency_model::SourcePackageFormat::Rpm,
                        record_index: 0,
                    },
                },
            ],
        );
        self
    }

    fn with_lookup(mut self, requested: &str, lookup: PackageLookup) -> Self {
        self.lookups.insert(requested.to_string(), lookup);
        self
    }
}

impl NativePackageSource for FixtureSource {
    fn manager(&self) -> SystemPackageManager {
        SystemPackageManager::Rpm
    }

    fn lookup(&self, requested: &str) -> PackageLookup {
        self.lookups
            .get(requested)
            .cloned()
            .unwrap_or_else(|| PackageLookup::Missing {
                reason: format!("{requested} is absent"),
            })
    }

    fn query_files(&self, query_name: &str) -> Result<Vec<FileInfoTuple>> {
        Ok(self.files.get(query_name).cloned().unwrap_or_default())
    }

    fn query_requirements(&self, query_name: &str) -> Result<Vec<RepositoryRequirementGroup>> {
        Ok(self
            .requirements
            .get(query_name)
            .cloned()
            .unwrap_or_default())
    }

    fn query_provides(
        &self,
        identity: &InstalledPackageIdentity,
    ) -> Result<Vec<ProvidedCapability>> {
        Ok(self
            .provides
            .get(identity.selector())
            .cloned()
            .unwrap_or_default())
    }
}

fn file_tuple(path: &Path, mode: i32) -> FileInfoTuple {
    (
        path.to_string_lossy().into_owned(),
        fs::symlink_metadata(path)
            .map(|metadata| i64::try_from(metadata.len()).unwrap())
            .unwrap_or(0),
        mode,
        None,
        Some("root".to_string()),
        Some("root".to_string()),
        None,
        conary_core::packages::InstalledFileAbsencePolicy::Required,
    )
}

fn temp_db() -> (tempfile::TempDir, String) {
    let temp = tempfile::tempdir().unwrap();
    let db_path = temp.path().join("conary.db");
    db::init(&db_path).unwrap();
    (temp, db_path.to_string_lossy().into_owned())
}

fn seed_tracked_file(db_path: &str, package: &str, path: &Path) {
    let conn = db::open(db_path).unwrap();
    let mut trove = Trove::new(
        package.to_string(),
        "9.9.9".to_string(),
        TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    let trove_id = trove.insert(&conn).unwrap();
    let node = conary_core::generation::root_manifest::capture_existing_payload_node(path).unwrap();
    let content = if matches!(node.source.kind, PayloadNodeKind::Regular { .. }) {
        let bytes = fs::read(path).unwrap();
        Some(PayloadContentAuthority {
            sha256: conary_core::hash::sha256(&bytes),
            size: bytes.len() as u64,
        })
    } else {
        None
    };
    let mut file = FileEntry::new(path.to_string_lossy().into_owned(), node, content, trove_id);
    file.insert(&conn).unwrap();
}

fn seed_adopted_native_file(db_path: &str, package: &str, path: &Path) {
    let conn = db::open(db_path).unwrap();
    let mut trove = Trove::new_with_source(
        package.to_string(),
        "1.2.3-4".to_string(),
        TroveType::Package,
        InstallSource::AdoptedTrack,
        conary_core::repository::versioning::VersionScheme::Rpm,
    );
    trove.architecture = Some("x86_64".to_string());
    trove.native_package_identity = Some(
        InstalledPackageIdentity::rpm(
            format!("{package}-1.2.3-4.x86_64"),
            package,
            None,
            "1.2.3",
            "4",
            "x86_64",
        )
        .unwrap(),
    );
    let trove_id = trove.insert(&conn).unwrap();
    let node = conary_core::generation::root_manifest::capture_existing_payload_node(path).unwrap();
    let content = if matches!(node.source.kind, PayloadNodeKind::Regular { .. }) {
        let bytes = fs::read(path).unwrap();
        Some(PayloadContentAuthority {
            sha256: conary_core::hash::sha256(&bytes),
            size: bytes.len() as u64,
        })
    } else {
        None
    };
    let mut file = FileEntry::new(path.to_string_lossy().into_owned(), node, content, trove_id);
    file.insert(&conn).unwrap();
}

fn tree_snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut snapshot = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .filter(|entry| {
            !entry
                .path()
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("conary.db"))
        })
        .map(|entry| {
            (
                entry.path().strip_prefix(root).unwrap().to_path_buf(),
                fs::read(entry.path()).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    snapshot.sort_by(|left, right| left.0.cmp(&right.0));
    snapshot
}

#[derive(Debug, PartialEq, Eq)]
struct DatabaseSnapshot {
    schema: Vec<(String, String, Option<String>)>,
    rows: Vec<(String, Vec<Vec<Vec<u8>>>)>,
}

fn database_snapshot(db_path: &str) -> DatabaseSnapshot {
    use rusqlite::types::ValueRef;

    let conn = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .unwrap();
    let mut schema = conn
        .prepare(
            "SELECT type, name, sql FROM sqlite_schema
             WHERE name NOT LIKE 'sqlite_autoindex_%'
             ORDER BY type, name",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    schema.sort();

    let table_names = schema
        .iter()
        .filter(|(kind, _, _)| kind == "table")
        .map(|(_, name, _)| name.clone())
        .collect::<Vec<_>>();
    let mut rows = Vec::with_capacity(table_names.len());
    for table in table_names {
        let quoted = table.replace('"', "\"\"");
        let mut statement = conn
            .prepare(&format!("SELECT * FROM \"{quoted}\""))
            .unwrap();
        let column_count = statement.column_count();
        let mut table_rows = statement
            .query_map([], |row| {
                (0..column_count)
                    .map(|column| {
                        Ok(match row.get_ref(column)? {
                            ValueRef::Null => vec![0],
                            ValueRef::Integer(value) => {
                                let mut encoded = vec![1];
                                encoded.extend_from_slice(&value.to_be_bytes());
                                encoded
                            }
                            ValueRef::Real(value) => {
                                let mut encoded = vec![2];
                                encoded.extend_from_slice(&value.to_bits().to_be_bytes());
                                encoded
                            }
                            ValueRef::Text(value) => {
                                let mut encoded = vec![3];
                                encoded.extend_from_slice(value);
                                encoded
                            }
                            ValueRef::Blob(value) => {
                                let mut encoded = vec![4];
                                encoded.extend_from_slice(value);
                                encoded
                            }
                        })
                    })
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        table_rows.sort();
        rows.push((table, table_rows));
    }

    DatabaseSnapshot { schema, rows }
}

fn ready_package(plan: &PackageAdoptionPlan) -> &PlannedPackage {
    plan.outcomes
        .iter()
        .find_map(PackagePlanOutcome::ready_package)
        .expect("expected a ready package")
}

#[test]
fn plan_classifies_ready_tracked_missing_ambiguous_unsupported_and_conflict() {
    let (temp, db_path) = temp_db();
    let ready_path = temp.path().join("root/usr/bin/ready");
    let tracked_path = temp.path().join("root/usr/bin/tracked");
    let conflict_path = temp.path().join("root/usr/bin/conflict");
    fs::create_dir_all(ready_path.parent().unwrap()).unwrap();
    fs::write(&ready_path, b"ready").unwrap();
    fs::write(&tracked_path, b"tracked").unwrap();
    fs::write(&conflict_path, b"conflict").unwrap();
    seed_adopted_native_file(&db_path, "tracked", &tracked_path);
    seed_tracked_file(&db_path, "other-owner", &conflict_path);

    let source = FixtureSource::default()
        .with_ready("ready", "ready", file_tuple(&ready_path, 0o100755))
        .with_ready("tracked", "tracked", file_tuple(&tracked_path, 0o100755))
        .with_ready(
            "conflicting",
            "conflicting",
            file_tuple(&conflict_path, 0o100755),
        )
        .with_lookup(
            "missing",
            PackageLookup::Missing {
                reason: "not installed".to_string(),
            },
        )
        .with_lookup(
            "ambiguous",
            PackageLookup::Ambiguous {
                reason: "two native variants match".to_string(),
            },
        )
        .with_lookup(
            "unsupported",
            PackageLookup::Unsupported {
                reason: "metadata cannot be represented".to_string(),
            },
        );
    let conn = db::open(&db_path).unwrap();
    let packages = [
        "ready",
        "tracked",
        "missing",
        "ambiguous",
        "unsupported",
        "conflicting",
    ]
    .map(str::to_string);

    let plan = build_adoption_plan(&conn, &packages, AdoptionMode::Track, &source).unwrap();

    assert!(matches!(plan.outcomes[0], PackagePlanOutcome::Ready(_)));
    assert!(matches!(
        plan.outcomes[1],
        PackagePlanOutcome::AlreadyTracked { .. }
    ));
    assert!(matches!(
        plan.outcomes[2],
        PackagePlanOutcome::Missing { .. }
    ));
    assert!(matches!(
        plan.outcomes[3],
        PackagePlanOutcome::Ambiguous { .. }
    ));
    assert!(matches!(
        plan.outcomes[4],
        PackagePlanOutcome::Unsupported { .. }
    ));
    assert!(matches!(
        plan.outcomes[5],
        PackagePlanOutcome::Conflict { .. }
    ));
}

#[test]
fn preview_and_apply_share_identity_mode_and_record_plan() {
    let (temp, db_path) = temp_db();
    let live_file = temp.path().join("native-root/usr/bin/fixture");
    fs::create_dir_all(live_file.parent().unwrap()).unwrap();
    fs::write(&live_file, b"fixture").unwrap();
    let source =
        FixtureSource::default().with_ready("fixture", "fixture", file_tuple(&live_file, 0o100755));
    let packages = vec!["fixture".to_string()];

    cmd_adopt_with_source(&packages, &db_path, false, true, &source).unwrap();
    let conn = db::open(&db_path).unwrap();
    assert!(Trove::find_by_name(&conn, "fixture").unwrap().is_empty());
    drop(conn);

    cmd_adopt_with_source(&packages, &db_path, false, false, &source).unwrap();

    let conn = db::open(&db_path).unwrap();
    let troves = Trove::find_by_name(&conn, "fixture").unwrap();
    assert_eq!(troves.len(), 1);
    assert_eq!(troves[0].version, "1.2.3-4");
    assert_eq!(troves[0].architecture.as_deref(), Some("x86_64"));
    assert_eq!(troves[0].install_source, InstallSource::AdoptedTrack);
    let trove_id = troves[0].id.unwrap();
    assert_eq!(FileEntry::find_by_trove(&conn, trove_id).unwrap().len(), 1);
    assert_eq!(
        conary_core::db::models::InstalledRequirementAtom::find_by_trove(&conn, trove_id)
            .unwrap()
            .len(),
        1
    );
    let provides = ProvideEntry::find_by_trove(&conn, trove_id).unwrap();
    assert_eq!(provides.len(), 3);
    let config = provides
        .iter()
        .find(|provide| provide.capability == "config(fixture)")
        .expect("source-declared config capability must be persisted");
    assert_eq!(config.version.as_deref(), Some("1.2.3-4"));
    assert_eq!(
        config.version_relation,
        Some(conary_core::repository::dependency_model::ProvideVersionRelation::Equal)
    );
    assert_eq!(
        config.kind,
        conary_core::repository::dependency_model::RepositoryCapabilityKind::Generic
    );
    assert!(matches!(
        config.provenance,
        conary_core::repository::dependency_model::CapabilityProvenance::SourceDeclared {
            format: conary_core::repository::dependency_model::SourcePackageFormat::Rpm,
            record_index: 0,
        }
    ));
    let file = provides
        .iter()
        .find(|provide| provide.capability == live_file.to_string_lossy())
        .expect("materialized package path must be persisted as a file provider");
    assert_eq!(
        file.kind,
        conary_core::repository::dependency_model::RepositoryCapabilityKind::File
    );
    assert!(matches!(
        &file.provenance,
        conary_core::repository::dependency_model::CapabilityProvenance::SourceDerivedFile {
            format: conary_core::repository::dependency_model::SourcePackageFormat::Rpm,
            source_path,
        } if source_path == live_file.to_string_lossy().as_ref()
    ));
}

#[test]
fn preview_reads_committed_wal_rows_without_mutating_logical_state_or_other_paths() {
    let (temp, db_path) = temp_db();
    let mut writer = db::open(&db_path).unwrap();
    let mut trove = Trove::new(
        "wal-fixture".to_string(),
        "1.0.0".to_string(),
        TroveType::Package,
        conary_core::repository::versioning::VersionScheme::Conary,
    );
    db::transaction(&mut writer, |tx| trove.insert(tx).map(|_| ())).unwrap();
    let database_before = database_snapshot(&db_path);
    let before = tree_snapshot(temp.path());

    let preview = open_preview_db(&db_path).unwrap();

    assert_eq!(
        Trove::find_by_name(&preview, "wal-fixture").unwrap().len(),
        1
    );
    assert_eq!(database_snapshot(&db_path), database_before);
    assert_eq!(tree_snapshot(temp.path()), before);
}

#[test]
fn full_preview_leaves_database_cas_backups_hooks_generations_and_live_file_unchanged() {
    let (temp, db_path) = temp_db();
    let live_file = temp.path().join("native-root/usr/bin/fixture");
    fs::create_dir_all(live_file.parent().unwrap()).unwrap();
    fs::write(&live_file, b"fixture bytes").unwrap();
    for marker in [
        temp.path().join("objects/existing"),
        temp.path().join("backups/existing"),
        temp.path().join("hooks/existing"),
        temp.path().join("generations/1/existing"),
    ] {
        fs::create_dir_all(marker.parent().unwrap()).unwrap();
        fs::write(marker, b"keep").unwrap();
    }
    let source =
        FixtureSource::default().with_ready("fixture", "fixture", file_tuple(&live_file, 0o100755));
    let database_before = database_snapshot(&db_path);
    let before = tree_snapshot(temp.path());

    cmd_adopt_with_source(&["fixture".to_string()], &db_path, true, true, &source).unwrap();

    assert_eq!(database_snapshot(&db_path), database_before);
    assert_eq!(tree_snapshot(temp.path()), before);
    assert_eq!(fs::read(&live_file).unwrap(), b"fixture bytes");
}

#[test]
fn refused_conflict_preview_is_tree_read_only() {
    let (temp, db_path) = temp_db();
    let live_file = temp.path().join("native-root/usr/bin/conflict");
    fs::create_dir_all(live_file.parent().unwrap()).unwrap();
    fs::write(&live_file, b"fixture bytes").unwrap();
    seed_tracked_file(&db_path, "other-owner", &live_file);
    let source =
        FixtureSource::default().with_ready("fixture", "fixture", file_tuple(&live_file, 0o100755));
    let database_before = database_snapshot(&db_path);
    let before = tree_snapshot(temp.path());

    let error = cmd_adopt_with_source(&["fixture".to_string()], &db_path, false, true, &source)
        .unwrap_err();

    assert!(error.to_string().contains("No packages are eligible"));
    assert_eq!(database_snapshot(&db_path), database_before);
    assert_eq!(tree_snapshot(temp.path()), before);
}

#[test]
fn duplicate_aliases_resolving_to_one_native_package_are_ambiguous() {
    let (temp, db_path) = temp_db();
    let live_file = temp.path().join("native-root/usr/bin/fixture");
    fs::create_dir_all(live_file.parent().unwrap()).unwrap();
    fs::write(&live_file, b"fixture").unwrap();
    let source = FixtureSource::default()
        .with_ready("fixture", "fixture", file_tuple(&live_file, 0o100755))
        .with_ready(
            "fixture.x86_64",
            "fixture",
            file_tuple(&live_file, 0o100755),
        );
    let conn = db::open(&db_path).unwrap();

    let plan = build_adoption_plan(
        &conn,
        &["fixture".to_string(), "fixture.x86_64".to_string()],
        AdoptionMode::Track,
        &source,
    )
    .unwrap();

    assert_eq!(plan.ready_count(), 0);
    assert!(
        plan.outcomes
            .iter()
            .all(|outcome| matches!(outcome, PackagePlanOutcome::Ambiguous { .. }))
    );
}

#[test]
fn same_name_multiarch_variants_remain_two_actionable_packages() {
    let (temp, db_path) = temp_db();
    let amd64_file = temp.path().join("native-root/usr/lib64/libfixture.so");
    let i686_file = temp.path().join("native-root/usr/lib/libfixture.so");
    fs::create_dir_all(amd64_file.parent().unwrap()).unwrap();
    fs::create_dir_all(i686_file.parent().unwrap()).unwrap();
    fs::write(&amd64_file, b"amd64").unwrap();
    fs::write(&i686_file, b"i686").unwrap();
    let source = FixtureSource::default()
        .with_ready_arch(
            "fixture.x86_64",
            "fixture",
            "x86_64",
            file_tuple(&amd64_file, 0o100755),
        )
        .with_ready_arch(
            "fixture.i686",
            "fixture",
            "i686",
            file_tuple(&i686_file, 0o100755),
        );
    let conn = db::open(&db_path).unwrap();

    let plan = build_adoption_plan(
        &conn,
        &["fixture.x86_64".into(), "fixture.i686".into()],
        AdoptionMode::Track,
        &source,
    )
    .unwrap();

    assert_eq!(plan.ready_count(), 2);
    let selectors = plan
        .outcomes
        .iter()
        .filter_map(PackagePlanOutcome::ready_package)
        .map(|package| package.identity.native.selector())
        .collect::<Vec<_>>();
    assert_eq!(
        selectors,
        vec!["fixture-1.2.3-4.x86_64", "fixture-1.2.3-4.i686"]
    );
}

#[test]
fn shared_directories_are_retained_as_package_claims() {
    let (temp, db_path) = temp_db();
    let shared_dir = temp.path().join("native-root/usr/share");
    let package_file = shared_dir.join("fixture");
    fs::create_dir_all(&shared_dir).unwrap();
    fs::write(&package_file, b"fixture").unwrap();
    seed_tracked_file(&db_path, "directory-owner", &shared_dir);
    let source = FixtureSource::default().with_ready(
        "fixture",
        "fixture",
        file_tuple(&shared_dir, 0o040755),
    );
    let conn = db::open(&db_path).unwrap();

    let plan = build_adoption_plan(
        &conn,
        &["fixture".to_string()],
        AdoptionMode::Track,
        &source,
    )
    .unwrap();
    let package = ready_package(&plan);

    assert_eq!(package.files.len(), 1);
    assert_eq!(package.files[0].0, shared_dir.to_string_lossy());
}

#[test]
fn planned_packages_resolve_shared_directories_and_file_conflicts_before_apply() {
    let (temp, db_path) = temp_db();
    let shared_dir = temp.path().join("native-root/usr/share");
    let shared_file = shared_dir.join("fixture");
    fs::create_dir_all(&shared_dir).unwrap();
    fs::write(&shared_file, b"fixture").unwrap();
    let source = FixtureSource::default()
        .with_ready("first", "first", file_tuple(&shared_dir, 0o040755))
        .with_ready("second", "second", file_tuple(&shared_dir, 0o040755))
        .with_ready(
            "file-owner",
            "file-owner",
            file_tuple(&shared_file, 0o100755),
        )
        .with_ready(
            "conflicting",
            "conflicting",
            file_tuple(&shared_file, 0o100755),
        );
    let conn = db::open(&db_path).unwrap();

    let plan = build_adoption_plan(
        &conn,
        &[
            "first".to_string(),
            "second".to_string(),
            "file-owner".to_string(),
            "conflicting".to_string(),
        ],
        AdoptionMode::Track,
        &source,
    )
    .unwrap();

    assert!(matches!(plan.outcomes[0], PackagePlanOutcome::Ready(_)));
    let PackagePlanOutcome::Ready(second) = &plan.outcomes[1] else {
        panic!("second package should remain ready");
    };
    assert_eq!(second.files.len(), 1);
    assert_eq!(second.files[0].0, shared_dir.to_string_lossy());
    assert!(matches!(plan.outcomes[2], PackagePlanOutcome::Ready(_)));
    assert!(matches!(
        plan.outcomes[3],
        PackagePlanOutcome::Conflict { .. }
    ));
}
