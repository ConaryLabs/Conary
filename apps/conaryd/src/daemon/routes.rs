// apps/conaryd/src/daemon/routes.rs

//! Axum router configuration for conaryd.
//!
//! `routes.rs` is the stable public route hub. Child modules own router
//! assembly, route helpers, API DTOs, and endpoint handlers.

mod auth;
mod db;
mod errors;
mod events;
mod query;
mod router;
mod sse;
mod system;
#[cfg(test)]
pub(super) mod test_support;
mod transactions;
mod types;

pub use errors::{ApiError, ApiResult};
pub use router::build_router;
pub use types::{
    CreateTransactionRequest, CreateTransactionResponse, DependencyInfo, DryRunResponse,
    DryRunSummary, HealthResponse, HistoryEntry, PackageDetails, PackageOperationOptions,
    PackageOperationRequest, PackageSummary, SearchQuery, SharedState, TransactionDetails,
    TransactionListQuery, TransactionOperation, TransactionSummary, VersionResponse,
};
