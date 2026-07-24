// conary-core/src/db/migrations/mod.rs

//! The single supported pre-alpha database schema.

mod current;

pub use current::create_current_schema;
