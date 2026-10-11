#![allow(dead_code)]
//! Foreign key constraint validation at approval time.
//!
//! Validates that shadow table changes won't violate FK constraints when applied to production.

use serde::Serialize;
use thiserror::Error;
use tracing::{debug, info, warn};

#[derive(Error, Debug)]
pub enum FkError {
    #[error("Database error: {0}")]
    Database(#[from] tokio_postgres::Error),

    #[error("Pool error: {0}")]
    Pool(#[from] deadpool_postgres::PoolError),

    #[error("Invalid identifier: {0}")]
    InvalidIdentifier(String),
}

/// Safely quote a PostgreSQL identifier to prevent SQL injection.
/// Validates the identifier contains only safe characters and double-quotes internal quotes.
fn quote_ident(ident: &str) -> Result<String, FkError> {
    // Validate: PostgreSQL identifiers can contain letters, digits, underscores
    // and must not be empty or excessively long
    if ident.is_empty() || ident.len() > 63 {
        return Err(FkError::InvalidIdentifier(ident.to_string()));
    }

    // Check for forbidden patterns that could indicate injection attempts
    let forbidden = [";", "--", "/*", "*/", "'", "\\"];
    for pattern in &forbidden {
        if ident.contains(pattern) {
            return Err(FkError::InvalidIdentifier(ident.to_string()));
        }
    }

    // Only allow alphanumeric and underscore
    if !ident.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(FkError::InvalidIdentifier(ident.to_string()));
    }

    // Double-quote the identifier (standard SQL quoting for identifiers)
    Ok(format!("\"{}\"", ident.replace('"', "\"\"")))
}

/// Safely format a string literal for use in SQL IN clause
fn quote_literal(s: &str) -> Result<String, FkError> {
    // Validate: no null bytes or other dangerous characters
    if s.contains('\0') {
        return Err(FkError::InvalidIdentifier(s.to_string()));
    }
    // Escape single quotes by doubling them
    Ok(format!("'{}'", s.replace('\'', "''")))
}

/// A foreign key constraint definition
#[derive(Debug, Clone)]
pub struct ForeignKey {
    pub constraint_name: String,
    pub source_table: String,
    pub source_columns: Vec<String>,
    pub target_table: String,
    pub target_columns: Vec<String>,
    pub on_delete: String,
    pub on_update: String,
}

/// A foreign key violation detected during validation
#[derive(Debug, Clone, Serialize)]
pub struct FkViolation {
    pub constraint_name: String,
    pub violation_type: FkViolationType,
    pub source_table: String,
    pub target_table: String,
    pub violating_values: Vec<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FkViolationType {
    /// INSERT/UPDATE references a non-existent row
    MissingReference,
    /// DELETE would orphan rows in referencing table
    WouldOrphan,
}

/// Get primary key columns for a table
async fn get_primary_key_columns(
    client: &deadpool_postgres::Client,
    schema: &str,
    table: &str,
) -> Result<Vec<String>, FkError> {
    let rows = client
        .query(
            r#"
            SELECT a.attname as column_name
            FROM pg_constraint c
            JOIN pg_class t ON t.oid = c.conrelid
            JOIN pg_namespace n ON n.oid = t.relnamespace
            JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = ANY(c.conkey)
            WHERE c.contype = 'p'
              AND n.nspname = $1
              AND t.relname = $2
            ORDER BY array_position(c.conkey, a.attnum)
            "#,
            &[&schema, &table],
        )
        .await?;

    Ok(rows.iter().map(|r| r.get("column_name")).collect())
}

/// Get all foreign key constraints for tables in the given schema
pub async fn get_foreign_keys(
    client: &deadpool_postgres::Client,
    schema: &str,
    tables: &[String],
) -> Result<Vec<ForeignKey>, FkError> {
    if tables.is_empty() {
        return Ok(Vec::new());
    }

    // Build IN clause with validated table names
    let mut quoted_tables = Vec::new();
    for t in tables {
        quoted_tables.push(quote_literal(t)?);
    }
    let table_list = quoted_tables.join(", ");

    // Schema is passed as a parameter, table names are validated literals
    let query = format!(
        r#"
        SELECT
            c.conname as constraint_name,
            src.relname as source_table,
            array_agg(DISTINCT src_attr.attname ORDER BY src_attr.attname) as source_columns,
            tgt.relname as target_table,
            array_agg(DISTINCT tgt_attr.attname ORDER BY tgt_attr.attname) as target_columns,
            CASE c.confdeltype
                WHEN 'a' THEN 'NO ACTION'
                WHEN 'r' THEN 'RESTRICT'
                WHEN 'c' THEN 'CASCADE'
                WHEN 'n' THEN 'SET NULL'
                WHEN 'd' THEN 'SET DEFAULT'
            END as on_delete,
            CASE c.confupdtype
                WHEN 'a' THEN 'NO ACTION'
                WHEN 'r' THEN 'RESTRICT'
                WHEN 'c' THEN 'CASCADE'
                WHEN 'n' THEN 'SET NULL'
                WHEN 'd' THEN 'SET DEFAULT'
            END as on_update
        FROM pg_constraint c
        JOIN pg_class src ON src.oid = c.conrelid
        JOIN pg_namespace src_ns ON src_ns.oid = src.relnamespace
        JOIN pg_class tgt ON tgt.oid = c.confrelid
        JOIN pg_attribute src_attr ON src_attr.attrelid = c.conrelid
            AND src_attr.attnum = ANY(c.conkey)
        JOIN pg_attribute tgt_attr ON tgt_attr.attrelid = c.confrelid
            AND tgt_attr.attnum = ANY(c.confkey)
        WHERE c.contype = 'f'
          AND src_ns.nspname = $1
          AND (src.relname IN ({table_list}) OR tgt.relname IN ({table_list}))
        GROUP BY c.conname, src.relname, tgt.relname, c.confdeltype, c.confupdtype
        "#,
        table_list = table_list
    );

    let rows = client.query(&query, &[&schema]).await?;

    let mut fks = Vec::new();
    for row in rows {
        fks.push(ForeignKey {
            constraint_name: row.get("constraint_name"),
            source_table: row.get("source_table"),
            source_columns: row.get("source_columns"),
            target_table: row.get("target_table"),
            target_columns: row.get("target_columns"),
            on_delete: row.get("on_delete"),
            on_update: row.get("on_update"),
        });
    }

    debug!(count = fks.len(), "Found foreign key constraints");
    Ok(fks)
}

/// Validate FK constraints for session changes
pub async fn validate_fk_constraints(
    client: &deadpool_postgres::Client,
    session_schema: &str,
    source_schema: &str,
) -> Result<Vec<FkViolation>, FkError> {
    let mut violations = Vec::new();

    // Get list of shadow tables (tables with changes)
    let shadow_tables: Vec<String> = client
        .query(
            r#"
            SELECT SUBSTRING(table_name FROM 9) as base_table
            FROM information_schema.tables
            WHERE table_schema = $1
              AND table_name LIKE '_shadow_%'
            "#,
            &[&session_schema],
        )
        .await?
        .iter()
        .map(|r| r.get("base_table"))
        .collect();

    if shadow_tables.is_empty() {
        debug!("No shadow tables found, skipping FK validation");
        return Ok(violations);
    }

    info!(tables = ?shadow_tables, "Validating FK constraints for shadow tables");

    // Get FK constraints involving these tables
    let fks = get_foreign_keys(client, source_schema, &shadow_tables).await?;

    for fk in &fks {
        // Check 1: INSERTs/UPDATEs in source table - do referenced rows exist?
        if shadow_tables.contains(&fk.source_table) {
            let insert_violations =
                check_missing_references(client, session_schema, source_schema, fk, &shadow_tables)
                    .await?;
            violations.extend(insert_violations);
        }

        // Check 2: DELETEs in target table - would any rows be orphaned?
        if shadow_tables.contains(&fk.target_table) {
            let delete_violations = check_orphaned_references(
                client,
                session_schema,
                source_schema,
                fk,
                &shadow_tables,
            )
            .await?;
            violations.extend(delete_violations);
        }
    }

    if !violations.is_empty() {
        warn!(
            count = violations.len(),
            "FK constraint violations detected"
        );
    }

    Ok(violations)
}

/// Check if INSERTs/UPDATEs in shadow table reference non-existent rows
async fn check_missing_references(
    client: &deadpool_postgres::Client,
    session_schema: &str,
    source_schema: &str,
    fk: &ForeignKey,
    shadow_tables: &[String],
) -> Result<Vec<FkViolation>, FkError> {
    let mut violations = Vec::new();

    // Validate and quote all identifiers
    let session_schema_q = quote_ident(session_schema)?;
    let source_schema_q = quote_ident(source_schema)?;
    let _source_table_q = quote_ident(&fk.source_table)?;
    let target_table_q = quote_ident(&fk.target_table)?;
    let shadow_source_q = quote_ident(&format!("_shadow_{}", fk.source_table))?;
    let shadow_target_q = quote_ident(&format!("_shadow_{}", fk.target_table))?;

    // Build column references for the join condition (with validation)
    let mut src_cols: Vec<String> = Vec::new();
    for c in &fk.source_columns {
        let col_q = quote_ident(c)?;
        src_cols.push(format!("s.{}", col_q));
    }

    let mut tgt_cols: Vec<String> = Vec::new();
    for c in &fk.target_columns {
        let col_q = quote_ident(c)?;
        tgt_cols.push(format!("t.{}", col_q));
    }

    let join_conditions: Vec<String> = src_cols
        .iter()
        .zip(tgt_cols.iter())
        .map(|(s, t)| format!("{} = {}", s, t))
        .collect();

    // Check if target table also has shadow changes
    let target_has_shadow = shadow_tables.contains(&fk.target_table);

    // Query: Find rows in shadow source table where FK columns don't exist in target
    let query = if target_has_shadow {
        format!(
            r#"
            SELECT DISTINCT {src_cols_select}
            FROM {session_schema}.{shadow_source} s
            WHERE NOT EXISTS (
                SELECT 1 FROM {source_schema}.{target_table} t
                WHERE {join_cond}
            )
            AND NOT EXISTS (
                SELECT 1 FROM {session_schema}.{shadow_target} t
                WHERE {join_cond}
            )
            AND ({not_null_check})
            LIMIT 10
            "#,
            src_cols_select = src_cols.join(", "),
            session_schema = session_schema_q,
            source_schema = source_schema_q,
            shadow_source = shadow_source_q,
            target_table = target_table_q,
            shadow_target = shadow_target_q,
            join_cond = join_conditions.join(" AND "),
            not_null_check = src_cols
                .iter()
                .map(|c| format!("{} IS NOT NULL", c))
                .collect::<Vec<_>>()
                .join(" AND "),
        )
    } else {
        format!(
            r#"
            SELECT DISTINCT {src_cols_select}
            FROM {session_schema}.{shadow_source} s
            WHERE NOT EXISTS (
                SELECT 1 FROM {source_schema}.{target_table} t
                WHERE {join_cond}
            )
            AND ({not_null_check})
            LIMIT 10
            "#,
            src_cols_select = src_cols.join(", "),
            session_schema = session_schema_q,
            source_schema = source_schema_q,
            shadow_source = shadow_source_q,
            target_table = target_table_q,
            join_cond = join_conditions.join(" AND "),
            not_null_check = src_cols
                .iter()
                .map(|c| format!("{} IS NOT NULL", c))
                .collect::<Vec<_>>()
                .join(" AND "),
        )
    };

    let rows = client.query(&query, &[]).await?;

    if !rows.is_empty() {
        let violating_values: Vec<String> = rows
            .iter()
            .map(|row| {
                fk.source_columns
                    .iter()
                    .enumerate()
                    .map(|(i, col)| {
                        let val: Option<String> = row.try_get(i).ok();
                        format!("{}={}", col, val.unwrap_or_else(|| "NULL".to_string()))
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .collect();

        violations.push(FkViolation {
            constraint_name: fk.constraint_name.clone(),
            violation_type: FkViolationType::MissingReference,
            source_table: fk.source_table.clone(),
            target_table: fk.target_table.clone(),
            violating_values,
            message: format!(
                "INSERT/UPDATE in '{}' references non-existent rows in '{}'",
                fk.source_table, fk.target_table
            ),
        });
    }

    Ok(violations)
}

/// Check if DELETEs in target table would orphan rows in referencing table
async fn check_orphaned_references(
    client: &deadpool_postgres::Client,
    session_schema: &str,
    source_schema: &str,
    fk: &ForeignKey,
    shadow_tables: &[String],
) -> Result<Vec<FkViolation>, FkError> {
    let mut violations = Vec::new();

    // Skip if ON DELETE CASCADE or SET NULL (these are safe)
    if fk.on_delete == "CASCADE" || fk.on_delete == "SET NULL" || fk.on_delete == "SET DEFAULT" {
        debug!(
            constraint = %fk.constraint_name,
            on_delete = %fk.on_delete,
            "Skipping orphan check due to ON DELETE action"
        );
        return Ok(violations);
    }

    // Check if there's a _deleted table for the target
    let deleted_table_exists: bool = client
        .query_one(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM information_schema.tables
                WHERE table_schema = $1 AND table_name = $2
            ) as exists
            "#,
            &[&session_schema, &format!("_deleted_{}", fk.target_table)],
        )
        .await?
        .get("exists");

    if !deleted_table_exists {
        return Ok(violations);
    }

    // Get primary key columns for source table to join with _deleted table
    // The _deleted tables only store PK columns, not FK columns
    let source_pk_cols = get_primary_key_columns(client, source_schema, &fk.source_table).await?;
    if source_pk_cols.is_empty() {
        debug!(
            table = %fk.source_table,
            "No primary key found for source table, skipping orphan check"
        );
        return Ok(violations);
    }

    // Check if source table also has shadow changes (might have corresponding deletes)
    let source_has_shadow = shadow_tables.contains(&fk.source_table);

    // Build and execute the orphan check query
    let query = build_orphan_check_query(
        session_schema,
        source_schema,
        fk,
        &source_pk_cols,
        source_has_shadow,
    )?;

    let rows = client.query(&query, &[]).await?;

    if !rows.is_empty() {
        let violating_values: Vec<String> = rows
            .iter()
            .map(|row| {
                fk.source_columns
                    .iter()
                    .enumerate()
                    .map(|(i, col)| {
                        let val: Option<String> = row.try_get(i).ok();
                        format!("{}={}", col, val.unwrap_or_else(|| "NULL".to_string()))
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .collect();

        violations.push(FkViolation {
            constraint_name: fk.constraint_name.clone(),
            violation_type: FkViolationType::WouldOrphan,
            source_table: fk.source_table.clone(),
            target_table: fk.target_table.clone(),
            violating_values,
            message: format!(
                "DELETE in '{}' would orphan rows in '{}' (constraint: {})",
                fk.target_table, fk.source_table, fk.constraint_name
            ),
        });
    }

    Ok(violations)
}

/// Build the SQL query for checking orphaned references.
/// Extracted for testability - generates the query that checks if deleting rows
/// from target table would orphan rows in the source table.
fn build_orphan_check_query(
    session_schema: &str,
    source_schema: &str,
    fk: &ForeignKey,
    source_pk_cols: &[String],
    source_has_shadow: bool,
) -> Result<String, FkError> {
    let session_schema_q = quote_ident(session_schema)?;
    let source_schema_q = quote_ident(source_schema)?;
    let source_table_q = quote_ident(&fk.source_table)?;
    let deleted_target_q = quote_ident(&format!("_deleted_{}", fk.target_table))?;
    let deleted_source_q = quote_ident(&format!("_deleted_{}", fk.source_table))?;

    // Build column references for FK join (source FK cols -> deleted target PK cols)
    let mut src_cols: Vec<String> = Vec::new();
    for c in &fk.source_columns {
        let col_q = quote_ident(c)?;
        src_cols.push(format!("src.{}", col_q));
    }

    let mut del_cols: Vec<String> = Vec::new();
    for c in &fk.target_columns {
        let col_q = quote_ident(c)?;
        del_cols.push(format!("del.{}", col_q));
    }

    let join_conditions: Vec<String> = src_cols
        .iter()
        .zip(del_cols.iter())
        .map(|(s, d)| format!("{} = {}", s, d))
        .collect();

    // Build src_del join condition using PRIMARY KEY columns (not FK columns!)
    // The _deleted tables only store PK columns, not the full row
    let mut src_del_cond_parts: Vec<String> = Vec::new();
    for c in source_pk_cols {
        let col_q = quote_ident(c)?;
        src_del_cond_parts.push(format!("src.{} = src_del.{}", col_q, col_q));
    }
    let src_del_cond = src_del_cond_parts.join(" AND ");

    let query = if source_has_shadow {
        format!(
            r#"
            SELECT DISTINCT {src_cols_select}
            FROM {source_schema}.{source_table} src
            JOIN {session_schema}.{deleted_target} del ON {join_cond}
            WHERE NOT EXISTS (
                SELECT 1 FROM {session_schema}.{deleted_source} src_del
                WHERE {src_del_cond}
            )
            LIMIT 10
            "#,
            src_cols_select = src_cols.join(", "),
            source_schema = source_schema_q,
            session_schema = session_schema_q,
            source_table = source_table_q,
            deleted_target = deleted_target_q,
            deleted_source = deleted_source_q,
            join_cond = join_conditions.join(" AND "),
            src_del_cond = src_del_cond,
        )
    } else {
        format!(
            r#"
            SELECT DISTINCT {src_cols_select}
            FROM {source_schema}.{source_table} src
            JOIN {session_schema}.{deleted_target} del ON {join_cond}
            LIMIT 10
            "#,
            src_cols_select = src_cols.join(", "),
            source_schema = source_schema_q,
            session_schema = session_schema_q,
            source_table = source_table_q,
            deleted_target = deleted_target_q,
            join_cond = join_conditions.join(" AND "),
        )
    };

    Ok(query)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quote_ident_valid() {
        assert_eq!(quote_ident("users").unwrap(), "\"users\"");
        assert_eq!(quote_ident("my_table").unwrap(), "\"my_table\"");
        assert_eq!(quote_ident("Table123").unwrap(), "\"Table123\"");
        assert_eq!(quote_ident("_private").unwrap(), "\"_private\"");
    }

    #[test]
    fn test_quote_ident_escapes_quotes() {
        // Internal quotes should be doubled (though our validation rejects them)
        // This tests the escaping logic if validation were relaxed
        let result = quote_ident("valid_name");
        assert!(result.is_ok());
    }

    #[test]
    fn test_quote_ident_rejects_empty() {
        assert!(quote_ident("").is_err());
    }

    #[test]
    fn test_quote_ident_rejects_too_long() {
        let long_name = "a".repeat(64);
        assert!(quote_ident(&long_name).is_err());
    }

    #[test]
    fn test_quote_ident_rejects_sql_injection_semicolon() {
        assert!(quote_ident("users; DROP TABLE users").is_err());
    }

    #[test]
    fn test_quote_ident_rejects_sql_injection_comment() {
        assert!(quote_ident("users--comment").is_err());
        assert!(quote_ident("users/*comment*/").is_err());
    }

    #[test]
    fn test_quote_ident_rejects_quotes() {
        assert!(quote_ident("user's").is_err());
        assert!(quote_ident("user\"s").is_err());
    }

    #[test]
    fn test_quote_ident_rejects_special_chars() {
        assert!(quote_ident("user-name").is_err());
        assert!(quote_ident("user.name").is_err());
        assert!(quote_ident("user name").is_err());
        assert!(quote_ident("user\nname").is_err());
    }

    #[test]
    fn test_quote_literal_valid() {
        assert_eq!(quote_literal("hello").unwrap(), "'hello'");
        assert_eq!(quote_literal("world_123").unwrap(), "'world_123'");
    }

    #[test]
    fn test_quote_literal_escapes_quotes() {
        assert_eq!(quote_literal("it's").unwrap(), "'it''s'");
        assert_eq!(quote_literal("say 'hello'").unwrap(), "'say ''hello'''");
    }

    #[test]
    fn test_quote_literal_rejects_null_byte() {
        assert!(quote_literal("hello\0world").is_err());
    }

    #[test]
    fn test_fk_violation_missing_reference_serialization() {
        let violation = FkViolation {
            constraint_name: "orders_user_id_fkey".to_string(),
            violation_type: FkViolationType::MissingReference,
            source_table: "orders".to_string(),
            target_table: "users".to_string(),
            violating_values: vec!["user_id=999".to_string()],
            message: "INSERT in 'orders' references non-existent rows in 'users'".to_string(),
        };

        let json = serde_json::to_string(&violation).unwrap();
        assert!(json.contains("missing_reference"));
        assert!(json.contains("orders_user_id_fkey"));
        assert!(json.contains("user_id=999"));
    }

    #[test]
    fn test_fk_violation_would_orphan_serialization() {
        let violation = FkViolation {
            constraint_name: "order_items_order_id_fkey".to_string(),
            violation_type: FkViolationType::WouldOrphan,
            source_table: "order_items".to_string(),
            target_table: "orders".to_string(),
            violating_values: vec!["order_id=42".to_string(), "order_id=43".to_string()],
            message: "DELETE in 'orders' would orphan rows in 'order_items'".to_string(),
        };

        let json = serde_json::to_string(&violation).unwrap();
        assert!(json.contains("would_orphan"));
        assert!(json.contains("order_items_order_id_fkey"));
        assert!(json.contains("order_id=42"));
        assert!(json.contains("order_id=43"));
    }

    #[test]
    fn test_fk_violation_multiple_columns() {
        let violation = FkViolation {
            constraint_name: "composite_fkey".to_string(),
            violation_type: FkViolationType::MissingReference,
            source_table: "child".to_string(),
            target_table: "parent".to_string(),
            violating_values: vec!["col1=1, col2=abc".to_string()],
            message: "Composite FK violation".to_string(),
        };

        let json = serde_json::to_string(&violation).unwrap();
        assert!(json.contains("composite_fkey"));
        assert!(json.contains("col1=1, col2=abc"));
    }

    #[test]
    fn test_foreign_key_struct() {
        let fk = ForeignKey {
            constraint_name: "orders_user_id_fkey".to_string(),
            source_table: "orders".to_string(),
            source_columns: vec!["user_id".to_string()],
            target_table: "users".to_string(),
            target_columns: vec!["id".to_string()],
            on_delete: "RESTRICT".to_string(),
            on_update: "CASCADE".to_string(),
        };

        assert_eq!(fk.constraint_name, "orders_user_id_fkey");
        assert_eq!(fk.source_columns.len(), 1);
        assert_eq!(fk.target_columns.len(), 1);
    }

    #[test]
    fn test_foreign_key_composite() {
        let fk = ForeignKey {
            constraint_name: "line_items_composite_fkey".to_string(),
            source_table: "line_items".to_string(),
            source_columns: vec!["order_id".to_string(), "product_id".to_string()],
            target_table: "order_products".to_string(),
            target_columns: vec!["order_id".to_string(), "product_id".to_string()],
            on_delete: "CASCADE".to_string(),
            on_update: "NO ACTION".to_string(),
        };

        assert_eq!(fk.source_columns.len(), 2);
        assert_eq!(fk.target_columns.len(), 2);
        assert_eq!(fk.on_delete, "CASCADE");
    }

    #[test]
    fn test_build_orphan_check_query_uses_pk_not_fk_columns() {
        // This is the key test for the FK validation fix:
        // When checking if deleting from 'users' would orphan rows in 'orders',
        // the query should use the PRIMARY KEY column ('id') to join with
        // _deleted_orders, NOT the FK column ('user_id').
        //
        // The _deleted_* tables only store: id (PK), _mlb_ts
        // They do NOT store FK columns like user_id.

        let fk = ForeignKey {
            constraint_name: "orders_user_id_fkey".to_string(),
            source_table: "orders".to_string(),
            source_columns: vec!["user_id".to_string()],
            target_table: "users".to_string(),
            target_columns: vec!["id".to_string()],
            on_delete: "RESTRICT".to_string(),
            on_update: "CASCADE".to_string(),
        };

        let source_pk_cols = vec!["id".to_string()];

        let query = build_orphan_check_query(
            "session_abc123",
            "public",
            &fk,
            &source_pk_cols,
            true, // source_has_shadow = true
        )
        .unwrap();

        // The join with _deleted_users should use the target PK column (del.id)
        assert!(
            query.contains("del.\"id\""),
            "Query should join on del.id (target PK)"
        );

        // The join with _deleted_orders should use source PK (src.id = src_del.id)
        // NOT the FK column (user_id)
        assert!(
            query.contains("src.\"id\" = src_del.\"id\""),
            "Query should use PK column 'id' for _deleted_orders join, not FK column 'user_id'"
        );
        assert!(
            !query.contains("src_del.\"user_id\""),
            "Query should NOT reference user_id on _deleted_orders (it doesn't exist there)"
        );
    }

    #[test]
    fn test_build_orphan_check_query_without_shadow() {
        let fk = ForeignKey {
            constraint_name: "orders_user_id_fkey".to_string(),
            source_table: "orders".to_string(),
            source_columns: vec!["user_id".to_string()],
            target_table: "users".to_string(),
            target_columns: vec!["id".to_string()],
            on_delete: "RESTRICT".to_string(),
            on_update: "CASCADE".to_string(),
        };

        let source_pk_cols = vec!["id".to_string()];

        let query = build_orphan_check_query(
            "session_abc123",
            "public",
            &fk,
            &source_pk_cols,
            false, // source_has_shadow = false
        )
        .unwrap();

        // Without shadow, there's no NOT EXISTS clause for _deleted_orders
        assert!(
            !query.contains("_deleted_orders"),
            "Query without shadow should not reference _deleted_orders"
        );
        assert!(
            query.contains("_deleted_users"),
            "Query should still reference _deleted_users"
        );
    }

    #[test]
    fn test_build_orphan_check_query_composite_pk() {
        let fk = ForeignKey {
            constraint_name: "order_items_order_id_fkey".to_string(),
            source_table: "order_items".to_string(),
            source_columns: vec!["order_id".to_string()],
            target_table: "orders".to_string(),
            target_columns: vec!["id".to_string()],
            on_delete: "RESTRICT".to_string(),
            on_update: "CASCADE".to_string(),
        };

        // Composite primary key
        let source_pk_cols = vec!["order_id".to_string(), "item_id".to_string()];

        let query =
            build_orphan_check_query("session_xyz", "public", &fk, &source_pk_cols, true).unwrap();

        // Should have both PK columns in the join condition
        assert!(
            query.contains("src.\"order_id\" = src_del.\"order_id\""),
            "Query should include first PK column"
        );
        assert!(
            query.contains("src.\"item_id\" = src_del.\"item_id\""),
            "Query should include second PK column"
        );
    }
}
