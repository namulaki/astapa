use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::info;
use uuid::Uuid;

use super::diff::TableDiff;

#[derive(Debug, Error)]
pub enum AuditError {
    #[error("Database error: {0}")]
    Database(#[from] tokio_postgres::Error),

    #[error("Pool error: {0}")]
    Pool(#[from] deadpool_postgres::PoolError),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub id: i64,
    pub session_id: Uuid,
    pub project_id: String,
    pub table_name: String,
    pub operation: String,
    pub row_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_data: Option<serde_json::Value>,
    pub approved_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approved_by: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct AuditQuery {
    pub session_id: Option<Uuid>,
    pub project_id: Option<String>,
    pub table_name: Option<String>,
    pub row_id: Option<String>,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

fn extract_row_id(row: &serde_json::Value) -> String {
    row.get("id")
        .and_then(|v| {
            v.as_str()
                .map(String::from)
                .or_else(|| v.as_i64().map(|n| n.to_string()))
        })
        .unwrap_or_else(|| {
            row.as_object()
                .and_then(|obj| obj.values().next())
                .map(|v| v.to_string())
                .unwrap_or_default()
        })
}

pub async fn persist_audit_log(
    pool: &deadpool_postgres::Pool,
    session_id: Uuid,
    project_id: &str,
    tables: &[TableDiff],
    approved_by: Option<&str>,
) -> Result<usize, AuditError> {
    let client = pool.get().await?;
    let mut count = 0;
    let now = Utc::now();

    for table in tables {
        for row in &table.inserts {
            let row_id = extract_row_id(row);
            client
                .execute(
                    r#"
                    INSERT INTO _astapa_audit_log
                    (session_id, project_id, table_name, operation, row_id, old_data, new_data, approved_at, approved_by)
                    VALUES ($1, $2, $3, 'INSERT', $4, NULL, $5, $6, $7)
                    "#,
                    &[
                        &session_id,
                        &project_id,
                        &table.name,
                        &row_id,
                        row,
                        &now,
                        &approved_by,
                    ],
                )
                .await?;
            count += 1;
        }

        for change in &table.updates {
            client
                .execute(
                    r#"
                    INSERT INTO _astapa_audit_log
                    (session_id, project_id, table_name, operation, row_id, old_data, new_data, approved_at, approved_by)
                    VALUES ($1, $2, $3, 'UPDATE', $4, $5, $6, $7, $8)
                    "#,
                    &[
                        &session_id,
                        &project_id,
                        &table.name,
                        &change.row_id,
                        &change.before,
                        &change.after,
                        &now,
                        &approved_by,
                    ],
                )
                .await?;
            count += 1;
        }

        for row in &table.deletes {
            let row_id = extract_row_id(row);
            client
                .execute(
                    r#"
                    INSERT INTO _astapa_audit_log
                    (session_id, project_id, table_name, operation, row_id, old_data, new_data, approved_at, approved_by)
                    VALUES ($1, $2, $3, 'DELETE', $4, $5, NULL, $6, $7)
                    "#,
                    &[
                        &session_id,
                        &project_id,
                        &table.name,
                        &row_id,
                        row,
                        &now,
                        &approved_by,
                    ],
                )
                .await?;
            count += 1;
        }
    }

    info!(
        session_id = %session_id,
        audit_records = count,
        "Persisted audit log"
    );
    Ok(count)
}

pub async fn query_audit_log(
    pool: &deadpool_postgres::Pool,
    query: &AuditQuery,
) -> Result<Vec<AuditEntry>, AuditError> {
    let client = pool.get().await?;

    let mut sql = String::from(
        "SELECT id, session_id, project_id, table_name, operation, row_id,
                old_data, new_data, approved_at, approved_by
         FROM _astapa_audit_log WHERE 1=1",
    );

    // Use boxed trait objects for proper typed parameter binding
    let mut params: Vec<Box<dyn tokio_postgres::types::ToSql + Sync + Send>> = Vec::new();
    let mut param_idx = 1;

    if let Some(session_id) = &query.session_id {
        sql.push_str(&format!(" AND session_id = ${}", param_idx));
        params.push(Box::new(*session_id));
        param_idx += 1;
    }

    if let Some(project_id) = &query.project_id {
        sql.push_str(&format!(" AND project_id = ${}", param_idx));
        params.push(Box::new(project_id.clone()));
        param_idx += 1;
    }

    if let Some(table_name) = &query.table_name {
        sql.push_str(&format!(" AND table_name = ${}", param_idx));
        params.push(Box::new(table_name.clone()));
        param_idx += 1;
    }

    if let Some(row_id) = &query.row_id {
        sql.push_str(&format!(" AND row_id = ${}", param_idx));
        params.push(Box::new(row_id.clone()));
        param_idx += 1;
    }

    if let Some(from) = &query.from {
        sql.push_str(&format!(" AND approved_at >= ${}", param_idx));
        params.push(Box::new(*from));
        param_idx += 1;
    }

    if let Some(to) = &query.to {
        sql.push_str(&format!(" AND approved_at <= ${}", param_idx));
        params.push(Box::new(*to));
        param_idx += 1;
    }

    sql.push_str(" ORDER BY approved_at DESC, id DESC");

    // Parameterize LIMIT with proper i64 type
    let limit_val: i64 = query.limit.unwrap_or(100).min(1000);
    sql.push_str(&format!(" LIMIT ${}", param_idx));
    params.push(Box::new(limit_val));
    param_idx += 1;

    // Parameterize OFFSET with proper i64 type
    if let Some(offset) = query.offset {
        sql.push_str(&format!(" OFFSET ${}", param_idx));
        params.push(Box::new(offset));
    }

    let param_refs: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = params
        .iter()
        .map(|p| p.as_ref() as &(dyn tokio_postgres::types::ToSql + Sync))
        .collect();

    let rows = client.query(&sql, &param_refs[..]).await?;

    let entries = rows
        .iter()
        .map(|row| AuditEntry {
            id: row.get("id"),
            session_id: row.get("session_id"),
            project_id: row.get("project_id"),
            table_name: row.get("table_name"),
            operation: row.get("operation"),
            row_id: row.get("row_id"),
            old_data: row.get("old_data"),
            new_data: row.get("new_data"),
            approved_at: row.get("approved_at"),
            approved_by: row.get("approved_by"),
        })
        .collect();

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_row_id_with_numeric_id() {
        let row = serde_json::json!({"id": 123, "name": "test"});
        assert_eq!(extract_row_id(&row), "123");
    }

    #[test]
    fn test_extract_row_id_with_string_id() {
        let row = serde_json::json!({"id": "abc-123", "name": "test"});
        assert_eq!(extract_row_id(&row), "abc-123");
    }

    #[test]
    fn test_extract_row_id_fallback() {
        let row = serde_json::json!({"uuid": "xyz", "name": "test"});
        assert_eq!(extract_row_id(&row), "\"test\"");
    }
}
