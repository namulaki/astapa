use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{error, info};
use uuid::Uuid;

use crate::approval::conflict::{check_row_hash_conflicts, ConflictType};
use crate::approval::diff::{generate_session_diff, SessionDiff};
use crate::approval::fk::{validate_fk_constraints, FkViolationType};
use crate::session::{get_session_mutations, Session, SessionManager, SessionMode, SessionStatus};

// ============================================================================
// Application State
// ============================================================================

/// Application state shared across handlers
#[derive(Clone)]
pub struct AppState {
    pub session_manager: Arc<SessionManager>,
    /// Source schema for production data (default: "public")
    pub source_schema: String,
    /// Default session mode when not specified in request
    pub default_session_mode: SessionMode,
}

// ============================================================================
// Request Types
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    pub project_id: String,
    pub mode: Option<String>,
}

// ============================================================================
// Response Types
// ============================================================================

#[derive(Debug, Serialize)]
pub struct CreateSessionResponse {
    pub session_id: String,
    pub schema_name: String,
    pub mode: String,
    pub expires_at: DateTime<Utc>,
    pub connection_string: String,
}

#[derive(Debug, Serialize)]
pub struct SessionResponse {
    pub session_id: String,
    pub project_id: String,
    pub schema_name: String,
    pub status: String,
    pub mode: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct ApproveResponse {
    pub status: String,
    pub applied: usize,
}

#[derive(Debug, Serialize)]
pub struct ConflictResponse {
    pub status: String,
    pub conflicts: Vec<ConflictDetail>,
}

#[derive(Debug, Serialize)]
pub struct ConflictDetail {
    pub table: String,
    pub pk_value: String,
    pub conflict_type: String,
    pub captured_hash: String,
    pub current_hash: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct FkViolationResponse {
    pub status: String,
    pub violations: Vec<FkViolationDetail>,
}

#[derive(Debug, Serialize)]
pub struct FkViolationDetail {
    pub constraint_name: String,
    pub violation_type: String,
    pub source_table: String,
    pub target_table: String,
    pub violating_values: Vec<String>,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct MutationsResponse {
    pub session_id: String,
    pub mutations: Vec<Mutation>,
}

#[derive(Debug, Serialize)]
pub struct Mutation {
    pub table: String,
    pub operation: String,
    pub row_id: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct MessageResponse {
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

// ============================================================================
// Handlers
// ============================================================================

/// POST /sessions - Create a new session
pub async fn create_session(
    State(state): State<AppState>,
    Json(request): Json<CreateSessionRequest>,
) -> Result<(StatusCode, Json<CreateSessionResponse>), (StatusCode, Json<ErrorResponse>)> {
    // Parse mode from request or use default
    let mode = match &request.mode {
        Some(mode_str) => SessionMode::from_str(mode_str).ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!("Invalid mode: {}. Must be 'agentic' or 'wiper'", mode_str),
                }),
            )
        })?,
        None => state.default_session_mode,
    };

    let session = state
        .session_manager
        .create_with_mode(&request.project_id, mode)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

    Ok((
        StatusCode::CREATED,
        Json(CreateSessionResponse {
            session_id: session.id.to_string(),
            schema_name: session.schema_name.clone(),
            mode: session.mode.as_str().to_string(),
            expires_at: session.expires_at,
            connection_string: format!(
                "postgresql://postgres:postgres@localhost:5433/astapa_playground?options=-c%20astapa_session%3D{}",
                session.id
            ),
        }),
    ))
}

/// GET /sessions/:id - Get session details
pub async fn get_session(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<SessionResponse>, (StatusCode, Json<ErrorResponse>)> {
    let session = state.session_manager.get(id).await.map_err(|e| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    Ok(Json(SessionResponse {
        session_id: session.id.to_string(),
        project_id: session.project_id.clone(),
        schema_name: session.schema_name.clone(),
        status: session.status.as_str().to_string(),
        mode: session.mode.as_str().to_string(),
        created_at: session.created_at,
        expires_at: session.expires_at,
    }))
}

/// DELETE /sessions/:id - Delete session
pub async fn delete_session(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<MessageResponse>, (StatusCode, Json<ErrorResponse>)> {
    state.session_manager.destroy(id).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    Ok(Json(MessageResponse {
        message: format!("Session {} deleted", id),
    }))
}

/// POST /sessions/:id/propose - Submit session for review
pub async fn propose_session(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<MessageResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Get session to check mode
    let session = state.session_manager.get(id).await.map_err(|e| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    // Only agentic mode supports propose/approve workflow
    if !session.mode.requires_approval() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!(
                    "Propose is only available in agentic mode (current: {})",
                    session.mode.as_str()
                ),
            }),
        ));
    }

    state
        .session_manager
        .update_status(id, SessionStatus::PendingReview)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

    Ok(Json(MessageResponse {
        message: format!("Session {} submitted for review", id),
    }))
}

/// POST /sessions/:id/approve - Approve and apply session changes
pub async fn approve_session(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    // 1. Get session and verify it's in PendingReview status
    let session = match state.session_manager.get(id).await {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::NOT_FOUND,
                Json(
                    serde_json::to_value(ErrorResponse {
                        error: e.to_string(),
                    })
                    .unwrap(),
                ),
            );
        }
    };

    // Only agentic mode supports propose/approve workflow
    if !session.mode.requires_approval() {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::to_value(ErrorResponse {
                    error: format!(
                        "Approve is only available in agentic mode (current: {})",
                        session.mode.as_str()
                    ),
                })
                .unwrap(),
            ),
        );
    }

    if session.status != SessionStatus::PendingReview {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                serde_json::to_value(ErrorResponse {
                    error: format!(
                        "Session must be in pending_review status to approve (current: {})",
                        session.status.as_str()
                    ),
                })
                .unwrap(),
            ),
        );
    }

    // 2. Get database client from pool
    let client = match state.session_manager.pool().get().await {
        Ok(c) => c,
        Err(e) => {
            error!(error = %e, "Failed to get database connection");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(
                    serde_json::to_value(ErrorResponse {
                        error: "Database connection error".to_string(),
                    })
                    .unwrap(),
                ),
            );
        }
    };

    // 3. Check for conflicts using row hash comparison
    let conflicts = match check_row_hash_conflicts(&client, &session.schema_name, "public").await {
        Ok(c) => c,
        Err(e) => {
            error!(error = %e, session_id = %id, "Failed to check conflicts");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(
                    serde_json::to_value(ErrorResponse {
                        error: format!("Conflict check failed: {}", e),
                    })
                    .unwrap(),
                ),
            );
        }
    };

    // 4. If conflicts exist, return them to client
    if !conflicts.is_empty() {
        info!(
            session_id = %id,
            conflict_count = conflicts.len(),
            "Conflicts detected, cannot approve"
        );

        let conflict_details: Vec<ConflictDetail> = conflicts
            .into_iter()
            .map(|c| ConflictDetail {
                table: c.table,
                pk_value: c.pk_value,
                conflict_type: match c.conflict_type {
                    ConflictType::RowModified => "row_modified".to_string(),
                    ConflictType::RowDeleted => "row_deleted".to_string(),
                    ConflictType::InsertCollision => "insert_collision".to_string(),
                },
                captured_hash: c.captured_hash,
                current_hash: c.current_hash,
            })
            .collect();

        return (
            StatusCode::CONFLICT,
            Json(
                serde_json::to_value(ConflictResponse {
                    status: "conflicts".to_string(),
                    conflicts: conflict_details,
                })
                .unwrap(),
            ),
        );
    }

    // 5. Check for FK constraint violations
    let fk_violations = match validate_fk_constraints(&client, &session.schema_name, "public").await
    {
        Ok(v) => v,
        Err(e) => {
            error!(error = %e, session_id = %id, "Failed to check FK constraints");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(
                    serde_json::to_value(ErrorResponse {
                        error: format!("FK constraint check failed: {}", e),
                    })
                    .unwrap(),
                ),
            );
        }
    };

    // 6. If FK violations exist, return them to client
    if !fk_violations.is_empty() {
        info!(
            session_id = %id,
            violation_count = fk_violations.len(),
            "FK constraint violations detected, cannot approve"
        );

        let violation_details: Vec<FkViolationDetail> = fk_violations
            .into_iter()
            .map(|v| FkViolationDetail {
                constraint_name: v.constraint_name,
                violation_type: match v.violation_type {
                    FkViolationType::MissingReference => "missing_reference".to_string(),
                    FkViolationType::WouldOrphan => "would_orphan".to_string(),
                },
                source_table: v.source_table,
                target_table: v.target_table,
                violating_values: v.violating_values,
                message: v.message,
            })
            .collect();

        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(
                serde_json::to_value(FkViolationResponse {
                    status: "fk_violations".to_string(),
                    violations: violation_details,
                })
                .unwrap(),
            ),
        );
    }

    // 7. No conflicts or FK violations - apply changes from shadow to production
    let applied = match apply_shadow_to_production(&client, &session).await {
        Ok(count) => count,
        Err(e) => {
            error!(error = %e, session_id = %id, "Failed to apply changes");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(
                    serde_json::to_value(ErrorResponse {
                        error: format!("Failed to apply changes: {}", e),
                    })
                    .unwrap(),
                ),
            );
        }
    };

    // 6. Update session status to Approved
    if let Err(e) = state
        .session_manager
        .update_status(id, SessionStatus::Approved)
        .await
    {
        error!(error = %e, session_id = %id, "Failed to update session status");
        // Changes already applied, log error but continue
    }

    // 7. Drop the session schema after successful apply
    if let Err(e) = state
        .session_manager
        .schema_manager()
        .drop_session_schema(&session.schema_name)
        .await
    {
        error!(error = %e, session_id = %id, "Failed to drop session schema");
        // Not critical, schema can be cleaned up later
    }

    info!(
        session_id = %id,
        applied = applied,
        "Session approved and changes applied"
    );

    (
        StatusCode::OK,
        Json(
            serde_json::to_value(ApproveResponse {
                status: "approved".to_string(),
                applied,
            })
            .unwrap(),
        ),
    )
}

/// Apply shadow table changes to production
async fn apply_shadow_to_production(
    client: &deadpool_postgres::Client,
    session: &Session,
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    let mut total_applied = 0;
    let schema = &session.schema_name;

    // Discover shadow tables dynamically from the schema
    // (proxy creates them without updating session.tables)
    let shadow_tables: Vec<String> = client
        .query(
            r#"
            SELECT table_name
            FROM information_schema.tables
            WHERE table_schema = $1
              AND table_name LIKE '_shadow_%'
            "#,
            &[schema],
        )
        .await?
        .iter()
        .map(|r| r.get("table_name"))
        .collect();

    info!(
        session_id = %session.id,
        shadow_count = shadow_tables.len(),
        "Discovered shadow tables for approval"
    );

    for shadow_table_name in shadow_tables {
        // Extract base table name: "_shadow_users" -> "users"
        let table_name = shadow_table_name
            .strip_prefix("_shadow_")
            .unwrap_or(&shadow_table_name);

        // Get primary key from production table
        let pk_row = client
            .query_opt(
                r#"
                SELECT a.attname as column_name
                FROM pg_index i
                JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = ANY(i.indkey)
                JOIN pg_class c ON c.oid = i.indrelid
                JOIN pg_namespace n ON n.oid = c.relnamespace
                WHERE i.indisprimary
                  AND n.nspname = 'public'
                  AND c.relname = $1
                LIMIT 1
                "#,
                &[&table_name],
            )
            .await?;

        let pk: String = match pk_row {
            Some(row) => row.get("column_name"),
            None => {
                tracing::warn!(table = %table_name, "No primary key found, skipping");
                continue;
            }
        };

        // Get production table columns and detect sequence-backed ones
        // Excludes _mlb_op and _mlb_ts which are shadow-only tracking columns
        let column_info: Vec<(String, bool)> = client
            .query(
                r#"
                SELECT
                    column_name,
                    COALESCE(column_default LIKE 'nextval%', false) as is_sequence
                FROM information_schema.columns
                WHERE table_schema = 'public'
                  AND table_name = $1
                ORDER BY ordinal_position
                "#,
                &[&table_name],
            )
            .await?
            .iter()
            .map(|r| (r.get("column_name"), r.get("is_sequence")))
            .collect();

        if column_info.is_empty() {
            tracing::warn!(table = %table_name, "No columns found in production table, skipping");
            continue;
        }

        let all_columns: Vec<&str> = column_info.iter().map(|(c, _)| c.as_str()).collect();
        let is_pk_sequence = column_info.iter().any(|(c, is_seq)| c == &pk && *is_seq);

        // Build column list for SELECT (excludes shadow tracking columns)
        let col_list = all_columns
            .iter()
            .map(|c| format!(r#""{}""#, c))
            .collect::<Vec<_>>()
            .join(", ");

        // Apply INSERTs (rows in shadow not yet in prod)
        if is_pk_sequence {
            // Sequence PK: exclude PK column so Postgres assigns fresh IDs
            let insert_columns: Vec<&str> =
                all_columns.iter().filter(|c| *c != &pk).copied().collect();
            let insert_col_list = insert_columns
                .iter()
                .map(|c| format!(r#""{}""#, c))
                .collect::<Vec<_>>()
                .join(", ");

            let insert_sql = format!(
                r#"
                INSERT INTO public."{table}" ({insert_col_list})
                SELECT {insert_col_list} FROM "{schema}"."_shadow_{table}" s
                WHERE NOT EXISTS (
                    SELECT 1 FROM public."{table}" p WHERE p."{pk}" = s."{pk}"
                )
                "#,
                schema = schema,
                table = table_name,
                pk = pk,
                insert_col_list = insert_col_list,
            );
            let inserted = client.execute(&insert_sql, &[]).await?;
            total_applied += inserted as usize;

            if inserted > 0 {
                info!(table = %table_name, count = inserted, "Inserted rows with fresh sequence IDs");
            }
        } else {
            // Non-sequence PK: copy IDs directly
            let insert_sql = format!(
                r#"
                INSERT INTO public."{table}" ({col_list})
                SELECT {col_list} FROM "{schema}"."_shadow_{table}" s
                WHERE NOT EXISTS (
                    SELECT 1 FROM public."{table}" p WHERE p."{pk}" = s."{pk}"
                )
                "#,
                schema = schema,
                table = table_name,
                pk = pk,
                col_list = col_list,
            );
            let inserted = client.execute(&insert_sql, &[]).await?;
            total_applied += inserted as usize;

            if inserted > 0 {
                info!(table = %table_name, count = inserted, "Inserted rows");
            }
        }

        // Apply UPDATEs (rows in shadow that exist in prod)
        let non_pk_columns: Vec<&str> = all_columns.iter().filter(|c| *c != &pk).copied().collect();

        if !non_pk_columns.is_empty() {
            let set_clause = non_pk_columns
                .iter()
                .map(|col| format!(r#""{col}" = s."{col}""#))
                .collect::<Vec<_>>()
                .join(", ");

            let update_sql = format!(
                r#"
                UPDATE public."{table}" p
                SET {set_clause}
                FROM "{schema}"."_shadow_{table}" s
                WHERE p."{pk}" = s."{pk}"
                "#,
                schema = schema,
                table = table_name,
                pk = pk,
                set_clause = set_clause,
            );
            let updated = client.execute(&update_sql, &[]).await?;
            total_applied += updated as usize;

            if updated > 0 {
                info!(table = %table_name, count = updated, "Updated rows");
            }
        }

        // Apply DELETEs from _deleted_{table} if it exists
        let delete_sql = format!(
            r#"
            DELETE FROM public."{table}"
            WHERE "{pk}"::text IN (
                SELECT "{pk}"::text FROM "{schema}"."_deleted_{table}"
            )
            "#,
            schema = schema,
            table = table_name,
            pk = pk,
        );
        if let Ok(deleted) = client.execute(&delete_sql, &[]).await {
            if deleted > 0 {
                info!(table = %table_name, count = deleted, "Deleted rows");
                total_applied += deleted as usize;
            }
        }
    }

    Ok(total_applied)
}

/// POST /sessions/:id/reject - Reject and discard session
pub async fn reject_session(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<MessageResponse>, (StatusCode, Json<ErrorResponse>)> {
    // First update status to Rejected
    state
        .session_manager
        .update_status(id, SessionStatus::Rejected)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

    // Then destroy the session (drop schema and remove from store)
    state.session_manager.destroy(id).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Failed to cleanup session: {}", e),
            }),
        )
    })?;

    Ok(Json(MessageResponse {
        message: format!("Session {} rejected and discarded", id),
    }))
}

/// GET /sessions/:id/mutations - Get staged mutations for session
pub async fn get_mutations(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<MutationsResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Get session to retrieve schema name
    let session = state.session_manager.get(id).await.map_err(|e| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    // Query mutations from shadow and deleted tables
    let mutation_records =
        get_session_mutations(state.session_manager.pool(), &session.schema_name)
            .await
            .map_err(|e| {
                error!(session_id = %id, error = %e, "Failed to get mutations");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
            })?;

    // Convert to response format
    let mutations: Vec<Mutation> = mutation_records
        .into_iter()
        .map(|rec| Mutation {
            table: rec.table,
            operation: rec.operation,
            row_id: rec.row_id,
            timestamp: rec.timestamp,
        })
        .collect();

    info!(session_id = %id, mutation_count = mutations.len(), "Retrieved mutations");

    Ok(Json(MutationsResponse {
        session_id: id.to_string(),
        mutations,
    }))
}

/// GET /sessions/:id/diff - Get diff of staged changes for session
pub async fn get_session_diff(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<SessionDiff>, (StatusCode, Json<ErrorResponse>)> {
    // Get session
    let session = state.session_manager.get(id).await.map_err(|e| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    // Generate diff using pool
    let tables = generate_session_diff(
        state.session_manager.pool(),
        &session.schema_name,
        &state.source_schema,
    )
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Failed to generate diff: {}", e),
            }),
        )
    })?;

    Ok(Json(SessionDiff {
        session_id: id.to_string(),
        tables,
    }))
}

/// GET /health - Health check endpoint
pub async fn health_check() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "healthy".to_string(),
        timestamp: Utc::now(),
    })
}
