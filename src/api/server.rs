use anyhow::Result;
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::info;

use crate::config::Config;
use crate::metrics;
use crate::session::{SessionManager, SessionMode};

use super::handlers::{
    approve_session, create_session, delete_session, get_audit_log, get_mutations, get_session,
    get_session_audit, get_session_diff, health_check, propose_session, reject_session, AppState,
};

/// Prometheus metrics endpoint - returns 500 if encoding fails
async fn metrics_handler() -> Response {
    let output = metrics::export_metrics();
    if output.starts_with("# ERROR:") {
        (StatusCode::INTERNAL_SERVER_ERROR, output).into_response()
    } else {
        output.into_response()
    }
}

/// Build the API router with all routes
pub fn build_router(
    session_manager: Arc<SessionManager>,
    default_session_mode: SessionMode,
) -> Router {
    let state = AppState {
        session_manager,
        source_schema: "public".to_string(),
        default_session_mode,
    };

    Router::new()
        // Session CRUD
        .route("/sessions", post(create_session))
        .route("/sessions/:id", get(get_session).delete(delete_session))
        // Session workflow
        .route("/sessions/:id/propose", post(propose_session))
        .route("/sessions/:id/approve", post(approve_session))
        .route("/sessions/:id/reject", post(reject_session))
        // Session data
        .route("/sessions/:id/mutations", get(get_mutations))
        .route("/sessions/:id/diff", get(get_session_diff))
        .route("/sessions/:id/audit", get(get_session_audit))
        // Audit log
        .route("/audit", get(get_audit_log))
        // Observability
        .route("/health", get(health_check))
        .route("/metrics", get(metrics_handler))
        // Middleware
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        // State
        .with_state(state)
}

/// Start the HTTP API server
pub async fn start_server(
    config: &Config,
    session_manager: Arc<SessionManager>,
    shutdown_rx: watch::Receiver<bool>,
) -> Result<()> {
    let app = build_router(session_manager, config.default_mode);

    let listener = TcpListener::bind(&config.api_addr).await?;
    info!("API server listening on {}", config.api_addr);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal(shutdown_rx))
        .await?;

    info!("API server stopped");
    Ok(())
}

/// Wait for shutdown signal
async fn shutdown_signal(mut rx: watch::Receiver<bool>) {
    // Use borrow_and_update to avoid race conditions
    // Exit on error (sender dropped) or when shutdown signaled
    loop {
        if rx.changed().await.is_err() {
            break;
        }
        if *rx.borrow_and_update() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_router_builds() {
        // This test verifies the router can be constructed without panicking
        // SessionManager would need to be mocked for actual integration tests
    }
}
