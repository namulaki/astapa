use anyhow::Result;
use deadpool_postgres::{Config as PgConfig, Runtime};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod api;
mod approval;
mod config;
mod metrics;
mod proxy;
mod rewriter;
mod sandbox;
mod session;

fn redact_url(url: &str) -> String {
    url::Url::parse(url)
        .map(|mut u| {
            if u.password().is_some() {
                let _ = u.set_password(Some("***"));
            }
            u.to_string()
        })
        .unwrap_or_else(|_| "[invalid url]".to_string())
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "astapa_proxy=debug,info".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    dotenvy::dotenv().ok();

    let config = config::Config::from_env()?;

    tracing::info!("Starting Astapa proxy");
    tracing::info!("Proxy listening on {}", config.proxy_addr);
    tracing::info!("API listening on {}", config.api_addr);
    tracing::info!("Backend database: {}", redact_url(&config.database_url));

    // Create shutdown channel
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Create database pool
    let mut pg_config = PgConfig::new();
    pg_config.url = Some(config.database_url.clone());
    let pool = pg_config.create_pool(Some(Runtime::Tokio1), tokio_postgres::NoTls)?;

    // Create and initialize session manager
    let session_manager = Arc::new(session::SessionManager::new(
        pool.clone(),
        config.session_ttl_seconds,
    ));
    session_manager.init().await?;

    // Extract backend address from DATABASE_URL
    let backend_addr = extract_backend_addr(&config.database_url);

    // Start API server in background
    let api_config = config.clone();
    let api_session_mgr = Arc::clone(&session_manager);
    let api_shutdown_rx = shutdown_rx.clone();
    let api_handle = tokio::spawn(async move {
        if let Err(e) = api::start_server(&api_config, api_session_mgr, api_shutdown_rx).await {
            tracing::error!("API server error: {}", e);
        }
    });

    // Start proxy server
    let proxy_addr = config.proxy_addr.clone();
    let proxy_pool = pool.clone();
    let proxy_session_mgr = Arc::clone(&session_manager);
    let proxy_shutdown_rx = shutdown_rx.clone();
    let proxy_handle = tokio::spawn(async move {
        if let Err(e) = proxy::start_server(
            &proxy_addr,
            &backend_addr,
            proxy_pool,
            proxy_session_mgr,
            proxy_shutdown_rx,
        )
        .await
        {
            tracing::error!("Proxy server error: {}", e);
        }
    });

    // Start background cleanup task
    let cleanup_session_mgr = Arc::clone(&session_manager);
    let mut cleanup_shutdown_rx = shutdown_rx.clone();
    let cleanup_handle = tokio::spawn(async move {
        let interval = Duration::from_secs(60); // Run every minute
        loop {
            tokio::select! {
                _ = tokio::time::sleep(interval) => {
                    match cleanup_session_mgr.cleanup_expired().await {
                        Ok(count) if count > 0 => {
                            tracing::debug!(count, "Background cleanup completed");
                        }
                        Ok(_) => {} // No sessions to clean up
                        Err(e) => {
                            tracing::warn!(error = %e, "Background cleanup failed");
                        }
                    }
                }
                result = cleanup_shutdown_rx.changed() => {
                    // Exit on error (sender dropped) or when shutdown signaled
                    if result.is_err() || *cleanup_shutdown_rx.borrow_and_update() {
                        tracing::info!("Cleanup task shutting down");
                        break;
                    }
                }
            }
        }
    });

    // Wait for shutdown signal (or unexpected server stop)
    let mut api_handle = api_handle;
    let mut proxy_handle = proxy_handle;

    tokio::select! {
        _ = signal_shutdown() => {
            tracing::info!("Shutdown signal received");
        }
        _ = &mut api_handle => {
            tracing::warn!("API server stopped unexpectedly");
        }
        _ = &mut proxy_handle => {
            tracing::warn!("Proxy server stopped unexpectedly");
        }
    }

    // Signal all components to shut down
    tracing::info!("Initiating graceful shutdown...");
    let _ = shutdown_tx.send(true);

    // Wait for servers to drain connections (proxy has 30s timeout, give 35s total)
    let drain_timeout = Duration::from_secs(35);

    tracing::info!("Waiting for proxy to drain connections...");
    match tokio::time::timeout(drain_timeout, proxy_handle).await {
        Ok(Ok(())) => tracing::info!("Proxy server shutdown complete"),
        Ok(Err(e)) => tracing::error!(error = %e, "Proxy server task failed"),
        Err(_) => tracing::warn!("Proxy drain timeout reached"),
    }

    tracing::info!("Waiting for API server to stop...");
    match tokio::time::timeout(Duration::from_secs(5), api_handle).await {
        Ok(Ok(())) => tracing::info!("API server shutdown complete"),
        Ok(Err(e)) => tracing::error!(error = %e, "API server task failed"),
        Err(_) => tracing::warn!("API server shutdown timeout reached"),
    }

    // Wait for cleanup task to finish
    match tokio::time::timeout(Duration::from_secs(5), cleanup_handle).await {
        Ok(Ok(())) => tracing::info!("Cleanup task shutdown complete"),
        Ok(Err(e)) => tracing::error!(error = %e, "Cleanup task failed"),
        Err(_) => tracing::warn!("Cleanup task shutdown timeout reached"),
    }

    tracing::info!("Shutdown complete");
    Ok(())
}

/// Wait for SIGTERM or SIGINT
async fn signal_shutdown() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm =
            signal(SignalKind::terminate()).expect("Failed to install SIGTERM handler");
        let mut sigint = signal(SignalKind::interrupt()).expect("Failed to install SIGINT handler");
        tokio::select! {
            _ = sigterm.recv() => tracing::info!("Received SIGTERM"),
            _ = sigint.recv() => tracing::info!("Received SIGINT"),
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
        tracing::info!("Received Ctrl+C");
    }
}

/// Extract host:port from a PostgreSQL connection URL
fn extract_backend_addr(database_url: &str) -> String {
    // Parse postgresql://user:pass@host:port/db
    if let Some(after_at) = database_url.split('@').nth(1) {
        if let Some(host_port) = after_at.split('/').next() {
            return host_port.to_string();
        }
    }
    // Fallback
    "localhost:5432".to_string()
}
