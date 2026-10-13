//! Integration tests for core Astapa functionality.
//!
//! These tests verify the main features of the Postgres proxy including:
//! - Session schema management
//! - Database connection handling
//! - Configuration loading
//!
//! Run with: cargo test --test integration_test -- --ignored

mod test_config;

use test_config::{
    cleanup_test_schema, create_test_pool, generate_session_id, session_schema_name,
    setup_empty_schema, setup_test_schema,
};

#[tokio::test]
#[ignore] // Requires database
async fn test_pool_creation() {
    let pool = create_test_pool().await;
    let client = pool.get().await.expect("Should get client from pool");

    // Verify we can execute a simple query
    let row = client
        .query_one("SELECT 1 as value", &[])
        .await
        .expect("Should execute simple query");

    let value: i32 = row.get("value");
    assert_eq!(value, 1);
}

#[tokio::test]
#[ignore] // Requires database
async fn test_session_schema_creation() {
    let pool = create_test_pool().await;
    let session_id = generate_session_id();
    let schema_name = setup_test_schema(&pool, &session_id).await;

    let client = pool.get().await.unwrap();

    // Verify schema exists
    let row = client
        .query_one(
            "SELECT EXISTS(SELECT 1 FROM information_schema.schemata WHERE schema_name = $1)",
            &[&schema_name],
        )
        .await
        .expect("Should check schema existence");

    let exists: bool = row.get(0);
    assert!(exists, "Session schema should exist");

    // Verify _deleted_users table exists
    let row = client
        .query_one(
            "SELECT EXISTS(SELECT 1 FROM information_schema.tables WHERE table_schema = $1 AND table_name = '_deleted_users')",
            &[&schema_name],
        )
        .await
        .expect("Should check table existence");

    let table_exists: bool = row.get(0);
    assert!(table_exists, "_deleted_users table should exist");

    cleanup_test_schema(&pool, &schema_name).await;
}

#[tokio::test]
#[ignore] // Requires database
async fn test_schema_cleanup() {
    let pool = create_test_pool().await;
    let session_id = generate_session_id();
    let schema_name = setup_test_schema(&pool, &session_id).await;

    let client = pool.get().await.unwrap();

    // Verify schema exists
    let row = client
        .query_one(
            "SELECT EXISTS(SELECT 1 FROM information_schema.schemata WHERE schema_name = $1)",
            &[&schema_name],
        )
        .await
        .unwrap();
    let exists: bool = row.get(0);
    assert!(exists);

    // Clean up
    cleanup_test_schema(&pool, &schema_name).await;

    // Verify schema is gone
    let row = client
        .query_one(
            "SELECT EXISTS(SELECT 1 FROM information_schema.schemata WHERE schema_name = $1)",
            &[&schema_name],
        )
        .await
        .unwrap();
    let exists_after: bool = row.get(0);
    assert!(!exists_after, "Schema should be cleaned up");
}

#[tokio::test]
#[ignore] // Requires database
async fn test_test_data_setup() {
    let pool = create_test_pool().await;
    let session_id = generate_session_id();
    let schema_name = setup_test_schema(&pool, &session_id).await;

    let client = pool.get().await.unwrap();

    // Verify test user exists
    let row = client
        .query_one("SELECT name FROM users WHERE id = 1", &[])
        .await
        .expect("Test user should exist");

    let name: String = row.get("name");
    assert_eq!(name, "Test User");

    // Verify test order exists
    let row = client
        .query_one("SELECT user_id, amount FROM orders WHERE id = 1", &[])
        .await
        .expect("Test order should exist");

    let user_id: i32 = row.get("user_id");
    let amount: rust_decimal::Decimal = row.get("amount");
    assert_eq!(user_id, 1);
    assert_eq!(amount.to_string(), "100.00");

    cleanup_test_schema(&pool, &schema_name).await;
}

#[tokio::test]
#[ignore] // Requires database
async fn test_empty_schema_creation() {
    let pool = create_test_pool().await;
    let session_id = generate_session_id();
    let schema_name = setup_empty_schema(&pool, &session_id).await;

    let client = pool.get().await.unwrap();

    // Verify schema exists
    let row = client
        .query_one(
            "SELECT EXISTS(SELECT 1 FROM information_schema.schemata WHERE schema_name = $1)",
            &[&schema_name],
        )
        .await
        .unwrap();

    let exists: bool = row.get(0);
    assert!(exists, "Empty schema should exist");

    // Verify no tables in the schema
    let rows = client
        .query(
            "SELECT table_name FROM information_schema.tables WHERE table_schema = $1",
            &[&schema_name],
        )
        .await
        .unwrap();

    assert_eq!(rows.len(), 0, "Empty schema should have no tables");

    cleanup_test_schema(&pool, &schema_name).await;
}

#[tokio::test]
#[ignore] // Requires database
async fn test_shadow_tables_structure() {
    let pool = create_test_pool().await;
    let session_id = generate_session_id();
    let schema_name = setup_test_schema(&pool, &session_id).await;

    let client = pool.get().await.unwrap();

    // Verify _shadow_users has expected columns
    let rows = client
        .query(
            "SELECT column_name FROM information_schema.columns WHERE table_schema = $1 AND table_name = '_shadow_users' ORDER BY ordinal_position",
            &[&schema_name],
        )
        .await
        .unwrap();

    let columns: Vec<String> = rows.iter().map(|r| r.get("column_name")).collect();
    assert!(columns.contains(&"id".to_string()));
    assert!(columns.contains(&"name".to_string()));
    assert!(columns.contains(&"_mlb_op".to_string()));
    assert!(columns.contains(&"_mlb_ts".to_string()));

    // Verify _shadow_orders has expected columns
    let rows = client
        .query(
            "SELECT column_name FROM information_schema.columns WHERE table_schema = $1 AND table_name = '_shadow_orders' ORDER BY ordinal_position",
            &[&schema_name],
        )
        .await
        .unwrap();

    let columns: Vec<String> = rows.iter().map(|r| r.get("column_name")).collect();
    assert!(columns.contains(&"id".to_string()));
    assert!(columns.contains(&"user_id".to_string()));
    assert!(columns.contains(&"amount".to_string()));
    assert!(columns.contains(&"_mlb_op".to_string()));
    assert!(columns.contains(&"_mlb_ts".to_string()));

    cleanup_test_schema(&pool, &schema_name).await;
}

#[tokio::test]
#[ignore] // Requires database
async fn test_concurrent_session_isolation() {
    let pool = create_test_pool().await;

    // Create two separate sessions
    let session1_id = generate_session_id();
    let session2_id = generate_session_id();

    let schema1 = setup_test_schema(&pool, &session1_id).await;
    let schema2 = setup_test_schema(&pool, &session2_id).await;

    // Verify they have different schema names
    assert_ne!(schema1, schema2, "Sessions should have unique schemas");

    let client = pool.get().await.unwrap();

    // Verify both schemas exist
    let row = client
        .query_one(
            "SELECT COUNT(*) as count FROM information_schema.schemata WHERE schema_name IN ($1, $2)",
            &[&schema1, &schema2],
        )
        .await
        .unwrap();

    let count: i64 = row.get("count");
    assert_eq!(count, 2, "Both session schemas should exist");

    // Clean up both
    cleanup_test_schema(&pool, &schema1).await;
    cleanup_test_schema(&pool, &schema2).await;
}

#[test]
fn test_session_schema_name_format() {
    let session_id = "test-session-123";
    let schema_name = session_schema_name(session_id);

    // Verify hyphens are replaced with underscores
    assert_eq!(schema_name, "session_test_session_123");
    assert!(!schema_name.contains('-'), "Schema name should not contain hyphens");
}

#[test]
fn test_generate_session_id_format() {
    let session_id = generate_session_id();

    // Verify it's a valid UUID format (contains hyphens)
    assert!(session_id.contains('-'), "Session ID should be UUID format");
    assert!(session_id.len() > 30, "Session ID should be reasonable length");
}
