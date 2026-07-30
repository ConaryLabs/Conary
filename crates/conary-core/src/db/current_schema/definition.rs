// conary-core/src/db/current_schema/definition.rs

use crate::error::Result;
use rusqlite::Connection;

const PACKAGE_MANAGER_SCHEMA: &str = include_str!("sql/package_manager.sql");
const PAYLOAD_CLAIMS_SCHEMA: &str = include_str!("sql/payload_claims.sql");
const REPOSITORY_SCHEMA: &str = include_str!("sql/repository.sql");
const REMI_SCHEMA: &str = include_str!("sql/remi.sql");

pub fn create_current_schema(conn: &Connection) -> Result<()> {
    for schema in [
        PACKAGE_MANAGER_SCHEMA,
        PAYLOAD_CLAIMS_SCHEMA,
        REPOSITORY_SCHEMA,
        REMI_SCHEMA,
    ] {
        conn.execute_batch(schema)?;
    }
    Ok(())
}
