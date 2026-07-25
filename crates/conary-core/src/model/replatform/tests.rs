// conary-core/src/model/replatform/tests.rs

use super::*;
use crate::db::models::{
    InstallSource, LabelEntry, PackageResolution, PrimaryStrategy, ProvideEntry, Repository,
    RepositoryPackage, RepositoryProvide, RepositoryRequirement, ResolutionStrategy,
    SystemAffinity, Trove, TroveType,
};
use crate::db::testing::create_test_db;
use crate::model::state::{InstalledPackage, SystemState};
use crate::repository::versioning::VersionScheme;

#[path = "execution_tests.rs"]
mod execution_tests;
#[path = "requirement_tests.rs"]
mod requirement_tests;
#[path = "snapshot_tests.rs"]
mod snapshot_tests;
