//! Database connectivity, migration, and readiness checks.

mod migrate;

pub use migrate::{MIGRATOR, check_connection, check_migrations_current, create_pool, migrate};
