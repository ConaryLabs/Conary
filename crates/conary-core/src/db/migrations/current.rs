// conary-core/src/db/migrations/current.rs

use crate::error::Result;
use rusqlite::Connection;

const PACKAGE_MANAGER_SCHEMA: &str = include_str!("current/package_manager.sql");
const REPOSITORY_SCHEMA: &str = include_str!("current/repository.sql");
const REMI_SCHEMA: &str = include_str!("current/remi.sql");

pub fn create_current_schema(conn: &Connection) -> Result<()> {
    for schema in [PACKAGE_MANAGER_SCHEMA, REPOSITORY_SCHEMA, REMI_SCHEMA] {
        conn.execute_batch(schema)?;
    }
    Ok(())
}
