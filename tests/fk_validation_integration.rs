//! Integration tests for FK constraint validation.
//!
//! These tests require a running PostgreSQL instance.
//! Set DATABASE_URL environment variable or use default: postgres://postgres@localhost/astapa_test
//!
//! Run with: cargo test --test fk_validation_integration -- --ignored

use deadpool_postgres::{Config, ManagerConfig, Pool, RecyclingMethod, Runtime};
use tokio_postgres::NoTls;
use uuid::Uuid;

async fn create_test_pool() -> Pool {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres@localhost/astapa_test".to_string());

    let mut config = Config::new();

    // Parse the DATABASE_URL manually
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
        .expect("Failed to create pool")
}

async fn setup_test_schema(pool: &Pool, session_id: &str) -> String {
    let client = pool.get().await.unwrap();
    let schema_name = format!("session_{}", session_id.replace('-', "_"));

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
        .unwrap();

    // Insert test data
    client
        .execute(
            "INSERT INTO users (id, name) VALUES (1, 'Test User') ON CONFLICT DO NOTHING",
            &[],
        )
        .await
        .unwrap();
    client
        .execute(
            "INSERT INTO orders (id, user_id, amount) VALUES (1, 1, 100.00) ON CONFLICT DO NOTHING",
            &[],
        )
        .await
        .unwrap();

    // Create session schema
    client
        .execute(
            &format!("CREATE SCHEMA IF NOT EXISTS \"{}\"", schema_name),
            &[],
        )
        .await
        .unwrap();

    // Create _deleted_users table (simulating a user deletion)
    client
        .execute(
            &format!(
                r#"
                CREATE TABLE IF NOT EXISTS "{}._deleted_users" (
                    id INTEGER NOT NULL,
                    _mlb_ts TIMESTAMPTZ DEFAULT NOW()
                )
                "#,
                schema_name
            ),
            &[],
        )
        .await
        .unwrap();

    // Create _deleted_orders table
    client
        .execute(
            &format!(
                r#"
                CREATE TABLE IF NOT EXISTS "{}._deleted_orders" (
                    id INTEGER NOT NULL,
                    _mlb_ts TIMESTAMPTZ DEFAULT NOW()
                )
                "#,
                schema_name
            ),
            &[],
        )
        .await
        .unwrap();

    // Create _shadow_users and _shadow_orders tables
    client
        .execute(
            &format!(
                r#"
                CREATE TABLE IF NOT EXISTS "{}._shadow_users" (
                    id INTEGER NOT NULL,
                    name TEXT,
                    _mlb_op TEXT,
                    _mlb_ts TIMESTAMPTZ DEFAULT NOW()
                )
                "#,
                schema_name
            ),
            &[],
        )
        .await
        .unwrap();

    client
        .execute(
            &format!(
                r#"
                CREATE TABLE IF NOT EXISTS "{}._shadow_orders" (
                    id INTEGER NOT NULL,
                    user_id INTEGER,
                    amount DECIMAL(10,2),
                    _mlb_op TEXT,
                    _mlb_ts TIMESTAMPTZ DEFAULT NOW()
                )
                "#,
                schema_name
            ),
            &[],
        )
        .await
        .unwrap();

    schema_name
}

async fn cleanup_test_schema(pool: &Pool, schema_name: &str) {
    let client = pool.get().await.unwrap();
    client
        .execute(
            &format!("DROP SCHEMA IF EXISTS \"{}\" CASCADE", schema_name),
            &[],
        )
        .await
        .ok();
}

#[tokio::test]
#[ignore] // Run with: cargo test --test fk_validation_integration -- --ignored
async fn test_fk_validation_detects_orphan_when_deleting_referenced_user() {
    let pool = create_test_pool().await;
    let session_id = Uuid::new_v4().to_string();
    let schema_name = setup_test_schema(&pool, &session_id).await;

    let client = pool.get().await.unwrap();

    // Simulate deleting user 1 (who has orders referencing them)
    client
        .execute(
            &format!(
                "INSERT INTO \"{}\"._deleted_users (id) VALUES (1)",
                schema_name
            ),
            &[],
        )
        .await
        .unwrap();

    // Now test that FK validation would detect this as an orphan violation
    // The _deleted_users table has user id=1, but orders.user_id=1 still exists
    // This should be detected as a would_orphan violation

    // Query the FK constraints
    let fk_rows = client
        .query(
            r#"
            SELECT
                c.conname as constraint_name,
                src.relname as source_table,
                tgt.relname as target_table
            FROM pg_constraint c
            JOIN pg_class src ON src.oid = c.conrelid
            JOIN pg_namespace ns ON ns.oid = src.relnamespace
            JOIN pg_class tgt ON tgt.oid = c.confrelid
            WHERE c.contype = 'f'
              AND ns.nspname = 'public'
              AND tgt.relname = 'users'
            "#,
            &[],
        )
        .await
        .unwrap();

    assert!(
        !fk_rows.is_empty(),
        "Should have FK constraint on orders -> users"
    );

    // Verify the orphan check query would work correctly
    // The key is that it uses the PK column (id) not the FK column (user_id)
    let orphan_check = client
        .query(
            &format!(
                r#"
                SELECT DISTINCT src."user_id"
                FROM public.orders src
                JOIN "{}"._deleted_users del ON src."user_id" = del."id"
                WHERE NOT EXISTS (
                    SELECT 1 FROM "{}"._deleted_orders src_del
                    WHERE src."id" = src_del."id"
                )
                LIMIT 10
                "#,
                schema_name, schema_name
            ),
            &[],
        )
        .await
        .unwrap();

    // Should find that order with user_id=1 would be orphaned
    assert_eq!(orphan_check.len(), 1, "Should detect 1 orphaned order");
    let user_id: i32 = orphan_check[0].get(0);
    assert_eq!(user_id, 1, "Orphaned order should reference user_id=1");

    cleanup_test_schema(&pool, &schema_name).await;
}

#[tokio::test]
#[ignore]
async fn test_fk_validation_no_orphan_when_order_also_deleted() {
    let pool = create_test_pool().await;
    let session_id = Uuid::new_v4().to_string();
    let schema_name = setup_test_schema(&pool, &session_id).await;

    let client = pool.get().await.unwrap();

    // Simulate deleting user 1
    client
        .execute(
            &format!(
                "INSERT INTO \"{}\"._deleted_users (id) VALUES (1)",
                schema_name
            ),
            &[],
        )
        .await
        .unwrap();

    // Also delete the order that references this user
    client
        .execute(
            &format!(
                "INSERT INTO \"{}\"._deleted_orders (id) VALUES (1)",
                schema_name
            ),
            &[],
        )
        .await
        .unwrap();

    // The orphan check should NOT find violations because the order is also being deleted
    let orphan_check = client
        .query(
            &format!(
                r#"
                SELECT DISTINCT src."user_id"
                FROM public.orders src
                JOIN "{}"._deleted_users del ON src."user_id" = del."id"
                WHERE NOT EXISTS (
                    SELECT 1 FROM "{}"._deleted_orders src_del
                    WHERE src."id" = src_del."id"
                )
                LIMIT 10
                "#,
                schema_name, schema_name
            ),
            &[],
        )
        .await
        .unwrap();

    assert_eq!(
        orphan_check.len(),
        0,
        "Should NOT detect orphans when order is also deleted"
    );

    cleanup_test_schema(&pool, &schema_name).await;
}

#[tokio::test]
#[ignore]
async fn test_deleted_table_only_has_pk_column() {
    let pool = create_test_pool().await;
    let session_id = Uuid::new_v4().to_string();
    let schema_name = setup_test_schema(&pool, &session_id).await;

    let client = pool.get().await.unwrap();

    // Verify that _deleted_orders only has id and _mlb_ts columns (no user_id!)
    let columns = client
        .query(
            r#"
            SELECT column_name
            FROM information_schema.columns
            WHERE table_schema = $1 AND table_name = '_deleted_orders'
            ORDER BY ordinal_position
            "#,
            &[&schema_name],
        )
        .await
        .unwrap();

    let column_names: Vec<String> = columns.iter().map(|r| r.get("column_name")).collect();

    assert!(
        column_names.contains(&"id".to_string()),
        "_deleted_orders should have 'id' column"
    );
    assert!(
        column_names.contains(&"_mlb_ts".to_string()),
        "_deleted_orders should have '_mlb_ts' column"
    );
    assert!(
        !column_names.contains(&"user_id".to_string()),
        "_deleted_orders should NOT have 'user_id' column - this is why the FK fix was needed"
    );

    cleanup_test_schema(&pool, &schema_name).await;
}
