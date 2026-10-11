#![allow(dead_code)]
use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use deadpool_postgres::Pool;
use thiserror::Error;
use tokio::sync::RwLock;
use tokio_postgres::Row;
use uuid::Uuid;

use super::manager::{Session, SessionMode, SessionStatus, TableState};

#[derive(Error, Debug)]
pub enum StoreError {
    #[error("Session not found: {0}")]
    NotFound(Uuid),

    #[error("Database error: {0}")]
    Database(#[from] tokio_postgres::Error),

    #[error("Pool error: {0}")]
    Pool(#[from] deadpool_postgres::PoolError),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

pub type StoreResult<T> = Result<T, StoreError>;

/// In-memory cache + Postgres persistence for sessions
pub struct SessionStore {
    pool: Pool,
    cache: Arc<RwLock<HashMap<Uuid, Session>>>,
}

impl SessionStore {
    pub fn new(pool: Pool) -> Self {
        Self {
            pool,
            cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Initialize the sessions metadata table
    pub async fn init(&self) -> StoreResult<()> {
        let client = self.pool.get().await?;

        client
            .execute(
                r#"
                CREATE TABLE IF NOT EXISTS _astapa_sessions (
                    id UUID PRIMARY KEY,
                    project_id TEXT NOT NULL,
                    schema_name TEXT NOT NULL,
                    status TEXT NOT NULL,
                    mode TEXT NOT NULL DEFAULT 'agentic',
                    created_at TIMESTAMPTZ NOT NULL,
                    expires_at TIMESTAMPTZ NOT NULL,
                    tables JSONB NOT NULL DEFAULT '{}'
                )
                "#,
                &[],
            )
            .await?;

        // Migration: add mode column if missing (for existing installations)
        client
            .execute(
                r#"
                ALTER TABLE _astapa_sessions
                ADD COLUMN IF NOT EXISTS mode TEXT NOT NULL DEFAULT 'agentic'
                "#,
                &[],
            )
            .await?;

        client
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_sessions_project ON _astapa_sessions(project_id)",
                &[],
            )
            .await?;

        client
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_sessions_expires ON _astapa_sessions(expires_at)",
                &[],
            )
            .await?;

        // Create audit log table for persistent mutation history
        client
            .execute(
                r#"
                CREATE TABLE IF NOT EXISTS _astapa_audit_log (
                    id BIGSERIAL PRIMARY KEY,
                    session_id UUID NOT NULL,
                    project_id TEXT NOT NULL,
                    table_name TEXT NOT NULL,
                    operation TEXT NOT NULL,
                    row_id TEXT NOT NULL,
                    old_data JSONB,
                    new_data JSONB,
                    approved_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                    approved_by TEXT
                )
                "#,
                &[],
            )
            .await?;

        // Audit log indexes
        client
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_audit_session_time ON _astapa_audit_log(session_id, approved_at DESC)",
                &[],
            )
            .await?;

        client
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_audit_project ON _astapa_audit_log(project_id)",
                &[],
            )
            .await?;

        client
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_audit_table ON _astapa_audit_log(table_name)",
                &[],
            )
            .await?;

        client
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_audit_row ON _astapa_audit_log(table_name, row_id)",
                &[],
            )
            .await?;

        client
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_audit_time ON _astapa_audit_log(approved_at)",
                &[],
            )
            .await?;

        Ok(())
    }

    /// Insert a new session into both cache and database
    pub async fn insert(&self, session: Session) -> StoreResult<()> {
        let client = self.pool.get().await?;

        let tables_json = serde_json::to_value(&session.tables)?;
        let status_str = session.status.as_str();
        let mode_str = session.mode.as_str();

        client
            .execute(
                r#"
                INSERT INTO _astapa_sessions (id, project_id, schema_name, status, mode, created_at, expires_at, tables)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                "#,
                &[
                    &session.id,
                    &session.project_id,
                    &session.schema_name,
                    &status_str,
                    &mode_str,
                    &session.created_at,
                    &session.expires_at,
                    &tables_json,
                ],
            )
            .await?;

        // Update cache
        let mut cache = self.cache.write().await;
        cache.insert(session.id, session);

        Ok(())
    }

    /// Get session from cache or load from database
    pub async fn get(&self, id: Uuid) -> StoreResult<Session> {
        // Check cache first
        {
            let cache = self.cache.read().await;
            if let Some(session) = cache.get(&id) {
                return Ok(session.clone());
            }
        }

        // Load from database
        let session = self.load_from_db(id).await?;

        // Update cache
        {
            let mut cache = self.cache.write().await;
            cache.insert(id, session.clone());
        }

        Ok(session)
    }

    /// Load session from database
    async fn load_from_db(&self, id: Uuid) -> StoreResult<Session> {
        let client = self.pool.get().await?;

        let row = client
            .query_opt(
                r#"
                SELECT id, project_id, schema_name, status, mode, created_at, expires_at, tables
                FROM _astapa_sessions
                WHERE id = $1
                "#,
                &[&id],
            )
            .await?
            .ok_or(StoreError::NotFound(id))?;

        Self::row_to_session(row)
    }

    /// Update session in both cache and database
    pub async fn update(&self, session: &Session) -> StoreResult<()> {
        let client = self.pool.get().await?;

        let tables_json = serde_json::to_value(&session.tables)?;
        let status_str = session.status.as_str();

        client
            .execute(
                r#"
                UPDATE _astapa_sessions
                SET status = $2, tables = $3, expires_at = $4
                WHERE id = $1
                "#,
                &[&session.id, &status_str, &tables_json, &session.expires_at],
            )
            .await?;

        // Update cache
        let mut cache = self.cache.write().await;
        cache.insert(session.id, session.clone());

        Ok(())
    }

    /// Delete session from both cache and database
    pub async fn delete(&self, id: Uuid) -> StoreResult<()> {
        let client = self.pool.get().await?;

        client
            .execute("DELETE FROM _astapa_sessions WHERE id = $1", &[&id])
            .await?;

        // Remove from cache
        let mut cache = self.cache.write().await;
        cache.remove(&id);

        Ok(())
    }

    /// List all sessions for a project
    pub async fn list_by_project(&self, project_id: &str) -> StoreResult<Vec<Session>> {
        let client = self.pool.get().await?;

        let rows = client
            .query(
                r#"
                SELECT id, project_id, schema_name, status, mode, created_at, expires_at, tables
                FROM _astapa_sessions
                WHERE project_id = $1
                ORDER BY created_at DESC
                "#,
                &[&project_id],
            )
            .await?;

        rows.into_iter().map(Self::row_to_session).collect()
    }

    /// List expired sessions
    pub async fn list_expired(&self) -> StoreResult<Vec<Session>> {
        let client = self.pool.get().await?;
        let now = Utc::now();

        let rows = client
            .query(
                r#"
                SELECT id, project_id, schema_name, status, mode, created_at, expires_at, tables
                FROM _astapa_sessions
                WHERE expires_at < $1 AND status = 'active'
                "#,
                &[&now],
            )
            .await?;

        rows.into_iter().map(Self::row_to_session).collect()
    }

    /// List sessions needing cleanup (expired active sessions OR any non-terminal sessions past TTL)
    pub async fn list_needing_cleanup(&self) -> StoreResult<Vec<Session>> {
        let client = self.pool.get().await?;
        let now = Utc::now();

        let rows = client
            .query(
                r#"
                SELECT id, project_id, schema_name, status, mode, created_at, expires_at, tables
                FROM _astapa_sessions
                WHERE (expires_at < $1 AND status IN ('active', 'pending_review'))
                   OR status IN ('expired', 'rejected')
                "#,
                &[&now],
            )
            .await?;

        rows.into_iter().map(Self::row_to_session).collect()
    }

    /// Invalidate cache entry
    pub async fn invalidate_cache(&self, id: Uuid) {
        let mut cache = self.cache.write().await;
        cache.remove(&id);
    }

    /// Clear entire cache
    pub async fn clear_cache(&self) {
        let mut cache = self.cache.write().await;
        cache.clear();
    }

    fn row_to_session(row: Row) -> StoreResult<Session> {
        let id: Uuid = row.get("id");
        let project_id: String = row.get("project_id");
        let schema_name: String = row.get("schema_name");
        let status_str: String = row.get("status");
        let mode_str: String = row.get("mode");
        let created_at: DateTime<Utc> = row.get("created_at");
        let expires_at: DateTime<Utc> = row.get("expires_at");
        let tables_json: serde_json::Value = row.get("tables");

        let status = SessionStatus::from_str(&status_str);
        let mode = SessionMode::from_str(&mode_str).unwrap_or_default();
        let tables: HashMap<String, TableState> = serde_json::from_value(tables_json)?;

        Ok(Session {
            id,
            project_id,
            schema_name,
            status,
            mode,
            created_at,
            expires_at,
            tables,
        })
    }
}
