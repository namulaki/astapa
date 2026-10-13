//! Common test configuration and utilities for integration tests.
//!
//! This module provides shared setup functions, test database pool creation,
//! and utility functions used across multiple test files.

use deadpool_postgres::{Config, ManagerConfig, Pool, RecyclingMethod, Runtime};
use tokio_postgres::NoTls;
use uuid::Uuid;

/// Creates a test database connection pool.
///
/// Uses DATABASE_URL environment variable if set, otherwise defaults to:
/// postgres://postgres@localhost/astapa_test
pub async fn create_test_pool() -> Pool {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres@localhost/astapa_test".to_string());

    let mut config = Config::new();

    // Parse the DATABASE_URL
    if let Ok(url) = url::Url::parse(&database_url) {
        config.host = url.host_str().map(String::from);
        config.port = url.port();
        config.user = if url.username().is_empty() {
            None
        } else {
            Some(url.username().to_string())
        };
        config.password = url.password().map(String::from);
        config.dbname = Some(url.path().trim_start_matches('/').to_string());
    }

    config.manager = Some(ManagerConfig {
        recycling_method: RecyclingMethod::Fast,
    });

    config
        .create_pool(Some(Runtime::Tokio1), NoTls)
        .expect("Failed to create test pool")
}

/// Generates a unique session ID for test isolation.
pub fn generate_session_id() -> String {
    Uuid::new_v4().to_string()
}

/// Creates a session schema name from a session ID.
///
/// Converts the session ID to a valid Postgres schema name by replacing
/// hyphens with underscores.
pub fn session_schema_name(session_id: &str) -> String {
    format!("session_{}", session_id.replace('-', "_"))
}

/// Sets up a basic test schema with users and orders tables.
///
/// Creates:
/// - public.users and public.orders tables with FK relationship
/// - Session-specific schema with _deleted and _shadow tables
/// - Test data (user id=1, order id=1)
///
/// Returns the schema name for cleanup.
pub async fn setup_test_schema(pool: &Pool, session_id: &str) -> String {
    let client = pool.get().await.expect("Failed to get client from pool");
    let schema_name = session_schema_name(session_id);

    // Create public schema tables if they don't exist
    client
        .batch_execute(
            r#"
            CREATE TABLE IF NOT EXISTS users (
                id SERIAL PRIMARY KEY,
                name TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS orders (
                id SERIAL PRIMARY KEY,
                user_id INTEGER NOT NULL REFERENCES users(id),
                amount DECIMAL(10,2)
            );
            "#,
        )
        .await
        .expect("Failed to create base tables");

    // Insert test data
    client
        .execute(
            "INSERT INTO users (id, name) VALUES (1, 'Test User') ON CONFLICT DO NOTHING",
            &[],
        )
        .await
        .expect("Failed to insert test user");

    client
        .execute(
            "INSERT INTO orders (id, user_id, amount) VALUES (1, 1, 100.00) ON CONFLICT DO NOTHING",
            &[],
        )
        .await
        .expect("Failed to insert test order");

    // Create session schema
    client
        .execute(
            &format!("CREATE SCHEMA IF NOT EXISTS \"{}\"", schema_name),
            &[],
        )
        .await
        .expect("Failed to create session schema");

    // Create _deleted tables
    client
        .batch_execute(&format!(
            r#"
            CREATE TABLE IF NOT EXISTS "{}._deleted_users" (
                id INTEGER NOT NULL,
                _mlb_ts TIMESTAMPTZ DEFAULT NOW()
            );

            CREATE TABLE IF NOT EXISTS "{}._deleted_orders" (
                id INTEGER NOT NULL,
                _mlb_ts TIMESTAMPTZ DEFAULT NOW()
            );
            "#,
            schema_name, schema_name
        ))
        .await
        .expect("Failed to create _deleted tables");

    // Create _shadow tables
    client
        .batch_execute(&format!(
            r#"
            CREATE TABLE IF NOT EXISTS "{}._shadow_users" (
                id INTEGER NOT NULL,
                name TEXT,
                _mlb_op TEXT,
                _mlb_ts TIMESTAMPTZ DEFAULT NOW()
            );

            CREATE TABLE IF NOT EXISTS "{}._shadow_orders" (
                id INTEGER NOT NULL,
                user_id INTEGER,
                amount DECIMAL(10,2),
                _mlb_op TEXT,
                _mlb_ts TIMESTAMPTZ DEFAULT NOW()
            );
            "#,
            schema_name, schema_name
        ))
        .await
        .expect("Failed to create _shadow tables");

    schema_name
}

/// Cleans up a test schema by dropping it with CASCADE.
pub async fn cleanup_test_schema(pool: &Pool, schema_name: &str) {
    let client = pool.get().await.expect("Failed to get client from pool");
    client
        .execute(
            &format!("DROP SCHEMA IF EXISTS \"{}\" CASCADE", schema_name),
            &[],
        )
        .await
        .ok(); // Ignore errors during cleanup
}

/// Creates a minimal session schema without test data.
///
/// Useful for tests that need a clean slate.
pub async fn setup_empty_schema(pool: &Pool, session_id: &str) -> String {
    let client = pool.get().await.expect("Failed to get client from pool");
    let schema_name = session_schema_name(session_id);

    client
        .execute(
            &format!("CREATE SCHEMA IF NOT EXISTS \"{}\"", schema_name),
            &[],
        )
        .await
        .expect("Failed to create empty schema");

    schema_name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_schema_name_conversion() {
        let session_id = "abc123-def-456";
        let schema_name = session_schema_name(session_id);
        assert_eq!(schema_name, "session_abc123_def_456");
    }

    #[test]
    fn test_generate_session_id_is_unique() {
        let id1 = generate_session_id();
        let id2 = generate_session_id();
        assert_ne!(id1, id2);
    }
}
